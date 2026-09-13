/* Salt Line — page behaviour. No dependencies, no trackers, no network. */
(() => {
  "use strict";

  // ---------------------------------------------------------- copy button
  document.querySelectorAll(".copy").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const src = document.querySelector(btn.dataset.copy);
      if (!src) return;
      const label = btn.textContent;
      try {
        await navigator.clipboard.writeText(src.innerText.trim());
        btn.textContent = "Copied";
      } catch {
        btn.textContent = "Select it";
        const range = document.createRange();
        range.selectNodeContents(src);
        const sel = window.getSelection();
        sel.removeAllRanges();
        sel.addRange(range);
      }
      setTimeout(() => (btn.textContent = label), 1600);
    });
  });

  // ------------------------------------------------------------ scrollspy
  const links = [...document.querySelectorAll('.topbar nav a[href^="#"]')];
  const targets = links
    .map((a) => document.querySelector(a.getAttribute("href")))
    .filter(Boolean);

  if (targets.length && "IntersectionObserver" in window) {
    const io = new IntersectionObserver(
      (entries) => {
        entries.forEach((e) => {
          if (!e.isIntersecting) return;
          links.forEach((a) =>
            a.toggleAttribute("aria-current", a.getAttribute("href") === "#" + e.target.id)
          );
        });
      },
      { rootMargin: "-45% 0px -50% 0px" }
    );
    targets.forEach((t) => io.observe(t));
  }

  // ------------------------------------------------- barrel angle readout
  // Mirrors the CSS sweep: 19 drawn frames, 5 degrees apart, alternating.
  // The sprite can only show the quantized angle; the field shows the true
  // one, which is the entire point of the Vernier modifier.
  const readout = document.getElementById("readout");
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  if (readout) {
    if (reduced) {
      readout.textContent = "BARREL 45° · TRUE 45.0°";
    } else {
      const FRAMES = 19;
      const STEP_MS = 200; // 3.8s / 19, matching the CSS animation
      let frame = 0;
      let dir = 1;
      let jitter = 0;
      setInterval(() => {
        frame += dir;
        if (frame >= FRAMES - 1 || frame <= 0) dir *= -1;
        jitter = (jitter + 1.7) % 5;
        const quantized = frame * 5;
        const trueAngle = Math.min(90, quantized + jitter);
        readout.textContent =
          "BARREL " + quantized + "° · TRUE " + trueAngle.toFixed(1) + "°";
      }, STEP_MS);
    }
  }
})();
