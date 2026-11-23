/* Salt Line — Black Friday / Cyber Monday 2026.
   The only place the window is defined. Times are America/Los_Angeles.
   `start` is inclusive (2026-11-27 00:00). `end` is exclusive at the next
   midnight, so the notice stays up through 2026-12-01 23:59.
   Outside that window the markup stays hidden and the page is unchanged.
   ?promo=bf2026 shows it for review, including after a dismissal.
   Dismissal is localStorage only. No cookies, no network. */
(function (root, factory) {
  "use strict";

  const api = factory();
  if (typeof module === "object" && module.exports) module.exports = api;
  root.SaltlinePromo = api;

  if (typeof document !== "undefined") {
    const start = function () {
      api.mount(document);
    };
    if (document.readyState === "loading") {
      document.addEventListener("DOMContentLoaded", start);
    } else {
      start();
    }
  }
})(typeof globalThis !== "undefined" ? globalThis : this, function () {
  "use strict";

  const PROMO = {
    preview: "bf2026",
    storageKey: "saltline.promo.bf2026.dismissed",
    timeZone: "America/Los_Angeles",
    start: "2026-11-27T00:00:00",
    end: "2026-12-02T00:00:00",
  };

  function zonedParts(date, timeZone) {
    const fmt = new Intl.DateTimeFormat("en-US", {
      timeZone: timeZone,
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      hourCycle: "h23",
    });
    const part = {};
    fmt.formatToParts(date).forEach(function (piece) {
      if (piece.type !== "literal") part[piece.type] = piece.value;
    });
    return part;
  }

  function stampFrom(part) {
    return (
      part.year + "-" + part.month + "-" + part.day +
      "T" + part.hour + ":" + part.minute + ":" + part.second
    );
  }

  // `following` is the wall time one second later, used only when an engine
  // reports midnight as 24:00 on the day that just ended.
  function wallStamp(part, following) {
    if (!part || part.hour !== "24") return stampFrom(part);
    return stampFrom({
      year: following.year,
      month: following.month,
      day: following.day,
      hour: "00",
      minute: "00",
      second: "00",
    });
  }

  // Local wall time as YYYY-MM-DDTHH:mm:ss, comparable as a string.
  function zonedStamp(date, timeZone) {
    const part = zonedParts(date, timeZone);
    const following = part.hour === "24"
      ? zonedParts(new Date(date.getTime() + 1000), timeZone)
      : part;
    return wallStamp(part, following);
  }

  function inWindow(now) {
    try {
      const stamp = zonedStamp(now, PROMO.timeZone);
      return stamp >= PROMO.start && stamp < PROMO.end;
    } catch (err) {
      return false;
    }
  }

  function previewOn(search) {
    try {
      return new URLSearchParams(search || "").get("promo") === PROMO.preview;
    } catch (err) {
      return false;
    }
  }

  function readDismissed(storage) {
    if (!storage || typeof storage.getItem !== "function") return false;
    try {
      return storage.getItem(PROMO.storageKey) === "1";
    } catch (err) {
      return false;
    }
  }

  function writeDismissed(storage) {
    if (!storage || typeof storage.setItem !== "function") return;
    try {
      storage.setItem(PROMO.storageKey, "1");
    } catch (err) {
      // Private mode still dismisses for this view; the markup is hidden below.
    }
  }

  // Preview forces a fresh look. A stored dismissal only applies to the real window.
  function shouldShow(now, search, storage) {
    if (previewOn(search)) return true;
    if (readDismissed(storage)) return false;
    return inWindow(now);
  }

  function applyCtas(doc) {
    const hero = doc.querySelector(".hero .cta");
    if (!hero) return;
    const source =
      hero.querySelector("a.btn.primary[href]") || hero.querySelector("a[href]");
    if (!source) return;
    const href = source.getAttribute("href");
    const label = (source.textContent || "").replace(/\s+/g, " ").trim();
    if (!href || !label) return;
    doc.querySelectorAll("a.promo-cta").forEach(function (el) {
      el.setAttribute("href", href);
      el.textContent = label;
    });
  }

  function hide(doc, storage) {
    writeDismissed(storage);
    const banner = doc.getElementById("bf2026");
    const note = doc.getElementById("bf2026-note");
    const active = doc.activeElement;
    const inside = [banner, note].some(function (el) {
      return !!(el && active && el.contains(active));
    });
    // Move focus before hiding. Hiding the focused control drops it on the
    // document body, and that move can land after a later focus() call.
    if (inside) {
      const brand = doc.querySelector(".brand");
      if (brand && typeof brand.focus === "function") brand.focus();
    }
    if (banner) banner.hidden = true;
    if (note) note.hidden = true;
  }

  function mount(doc, options) {
    const opts = options || {};
    const banner = doc.getElementById("bf2026");
    const note = doc.getElementById("bf2026-note");
    if (!banner && !note) return;
    const view = doc.defaultView || globalThis;
    let storage = opts.storage;
    if (!storage) {
      try {
        storage = view && view.localStorage;
      } catch (err) {
        storage = null;
      }
    }
    const search =
      opts.search != null ? opts.search : view && view.location ? view.location.search : "";
    const now = opts.now || new Date();
    if (!shouldShow(now, search, storage)) return;

    applyCtas(doc);
    if (banner) banner.hidden = false;
    if (note) note.hidden = false;

    const dismiss = function () {
      hide(doc, storage);
    };
    doc.querySelectorAll(".promo-dismiss").forEach(function (btn) {
      btn.addEventListener("click", dismiss);
    });
    doc.addEventListener("keydown", function (event) {
      if (event.key !== "Escape") return;
      if (!banner || banner.hidden) return;
      const tag = event.target && event.target.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
      event.preventDefault();
      dismiss();
    });
  }

  return {
    PROMO: PROMO,
    zonedStamp: zonedStamp,
    wallStamp: wallStamp,
    inWindow: inWindow,
    shouldShow: shouldShow,
    mount: mount,
  };
});
