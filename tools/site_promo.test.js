/* Date window and preview switch for site/promo.js. Run with: node --test */
"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const promo = require("../site/promo.js");

function memory(value) {
  return {
    getItem: function () {
      return value;
    },
  };
}

test("the window is Pacific time, inclusive of the last minute of Dec 1", function () {
  const show = function (iso) {
    return promo.shouldShow(new Date(iso), "", memory(null));
  };
  // 2026-11-26 23:59:59 PST
  assert.equal(show("2026-11-27T07:59:59Z"), false);
  // 2026-11-27 00:00:00 PST
  assert.equal(show("2026-11-27T08:00:00Z"), true);
  // midday during the weekend
  assert.equal(show("2026-11-29T20:00:00Z"), true);
  // 2026-12-01 23:59:59 PST
  assert.equal(show("2026-12-02T07:59:59Z"), true);
  // 2026-12-02 00:00:00 PST
  assert.equal(show("2026-12-02T08:00:00Z"), false);
});

test("the zone follows daylight time instead of a fixed offset", function () {
  assert.equal(
    promo.zonedStamp(new Date("2026-07-04T19:00:00Z"), promo.PROMO.timeZone),
    "2026-07-04T12:00:00"
  );
  assert.equal(
    promo.zonedStamp(new Date("2026-12-01T20:00:00Z"), promo.PROMO.timeZone),
    "2026-12-01T12:00:00"
  );
});

test("midnight reported as 24:00 uses the following second's calendar day", function () {
  assert.equal(
    promo.wallStamp(
      { year: "2026", month: "11", day: "26", hour: "24", minute: "00", second: "00" },
      { year: "2026", month: "11", day: "27", hour: "00", minute: "00", second: "01" }
    ),
    "2026-11-27T00:00:00"
  );
  assert.equal(
    promo.wallStamp(
      { year: "2026", month: "11", day: "27", hour: "00", minute: "00", second: "00" },
      { year: "2026", month: "11", day: "27", hour: "00", minute: "00", second: "01" }
    ),
    "2026-11-27T00:00:00"
  );
});

test("?promo=bf2026 shows the notice outside the window and after dismissal", function () {
  const january = new Date("2026-01-15T20:00:00Z");
  assert.equal(promo.shouldShow(january, "?promo=bf2026", memory("1")), true);
  assert.equal(promo.shouldShow(january, "?other=1&promo=bf2026", memory(null)), true);
  assert.equal(promo.shouldShow(january, "?promo=BF2026", memory(null)), false);
  assert.equal(promo.shouldShow(january, "?promo=bf2026x", memory(null)), false);
  assert.equal(promo.shouldShow(january, "", memory(null)), false);
});

test("a dismissal hides the real window and a broken store does not", function () {
  const during = new Date("2026-11-29T20:00:00Z");
  assert.equal(promo.shouldShow(during, "", memory("1")), false);
  const broken = {
    getItem: function () {
      throw new Error("denied");
    },
  };
  assert.equal(promo.shouldShow(during, "", broken), true);
  assert.equal(promo.shouldShow(new Date("2026-06-01T18:00:00Z"), "", broken), false);
});

test("mount reveals both regions and points their buttons at the live call to action", function () {
  const clicks = [];
  const keys = [];
  let focused = null;
  function element(tag, id) {
    const el = {
      tagName: tag.toUpperCase(),
      id: id,
      hidden: true,
      children: [],
      attrs: {},
      textContent: "",
      getAttribute: function (name) {
        return Object.prototype.hasOwnProperty.call(el.attrs, name) ? el.attrs[name] : null;
      },
      setAttribute: function (name, value) {
        el.attrs[name] = value;
      },
      addEventListener: function (type, fn) {
        clicks.push({ id: id, type: type, fn: fn });
      },
      contains: function (node) {
        return node === el || el.children.indexOf(node) !== -1;
      },
      focus: function () {
        focused = el;
      },
    };
    return el;
  }

  const banner = element("aside", "bf2026");
  const note = element("section", "bf2026-note");
  const dismiss = element("button", "");
  dismiss.hidden = false;
  banner.children.push(dismiss);
  const heroLink = element("a", "");
  heroLink.hidden = false;
  heroLink.attrs.href = "https://testflight.apple.com/join/PgjfndD9";
  heroLink.textContent = "Play free on iPhone (beta)";
  heroLink.className = "btn primary";
  const bannerCta = element("a", "");
  bannerCta.attrs.href = "#get";
  bannerCta.textContent = "Get Salt Line";
  const noteCta = element("a", "");
  noteCta.attrs.href = "#get";
  noteCta.textContent = "Get Salt Line";
  const brand = element("a", "");
  const stored = { value: null };

  const doc = {
    activeElement: dismiss,
    defaultView: null,
    getElementById: function (id) {
      if (id === "bf2026") return banner;
      if (id === "bf2026-note") return note;
      return null;
    },
    querySelector: function (sel) {
      if (sel === ".hero .cta") return { querySelector: doc.querySelector };
      if (sel === "a.btn.primary[href]" || sel === "a[href]") return heroLink;
      if (sel === ".brand") return brand;
      return null;
    },
    querySelectorAll: function (sel) {
      if (sel === "a.promo-cta") return [bannerCta, noteCta];
      if (sel === ".promo-dismiss") return [dismiss];
      return [];
    },
    addEventListener: function (type, fn) {
      keys.push({ type: type, fn: fn });
    },
  };

  promo.mount(doc, {
    now: new Date("2026-06-01T00:00:00Z"),
    search: "?promo=bf2026",
    storage: {
      getItem: function () {
        return stored.value;
      },
      setItem: function (_key, value) {
        stored.value = value;
      },
    },
  });

  assert.equal(banner.hidden, false);
  assert.equal(note.hidden, false);
  assert.equal(bannerCta.getAttribute("href"), heroLink.getAttribute("href"));
  assert.equal(bannerCta.textContent, "Play free on iPhone (beta)");
  assert.equal(noteCta.textContent, "Play free on iPhone (beta)");

  clicks[0].fn();
  assert.equal(stored.value, "1");
  assert.equal(banner.hidden, true);
  assert.equal(note.hidden, true);
  assert.equal(focused, brand);

  banner.hidden = false;
  doc.activeElement = { tagName: "INPUT" };
  const escape = { key: "Escape", target: doc.activeElement, preventDefault: function () { this.defaulted = true; } };
  keys[0].fn(escape);
  assert.equal(banner.hidden, false);
  assert.equal(escape.defaulted, undefined);
});

test("mount leaves the page alone outside the window", function () {
  const banner = { id: "bf2026", hidden: true, contains: function () { return false; } };
  const note = { id: "bf2026-note", hidden: true, contains: function () { return false; } };
  let queried = false;
  const doc = {
    getElementById: function (id) {
      return id === "bf2026" ? banner : note;
    },
    querySelector: function () {
      queried = true;
      return null;
    },
    querySelectorAll: function () {
      queried = true;
      return [];
    },
    addEventListener: function () {
      queried = true;
    },
    defaultView: null,
  };
  promo.mount(doc, {
    now: new Date("2026-11-27T07:59:59Z"),
    search: "",
    storage: memory(null),
  });
  assert.equal(banner.hidden, true);
  assert.equal(note.hidden, true);
  assert.equal(queried, false);
});
