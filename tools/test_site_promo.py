"""The marketing page's Black Friday notice stays inert outside its window."""

import re
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SITE = ROOT / "site"


class SitePromoTests(unittest.TestCase):
    def test_markup_stays_hidden_without_javascript(self) -> None:
        html = (SITE / "index.html").read_text(encoding="utf-8")
        for element_id in ("bf2026", "bf2026-note"):
            start = html.index(f'id="{element_id}"')
            tag = html.rfind("<", 0, start)
            opening = html[tag : html.index(">", start)]
            self.assertIn("hidden", opening)
            self.assertIn('aria-labelledby="', opening)
        self.assertIn('role="region"', html)
        self.assertIn('class="promo-dismiss"', html)
        self.assertIn('type="button"', html)
        self.assertEqual(
            re.findall(r'<script[^>]*\ssrc="([^"]+)"', html),
            ["main.js", "promo.js"],
        )
        self.assertNotIn("document.cookie", html.lower())

    def test_hidden_rules_keep_the_notice_out_of_flow(self) -> None:
        css = (SITE / "style.css").read_text(encoding="utf-8")
        rule = css.split(".promo[hidden]", 1)[1].split("}", 1)[0]
        self.assertIn(".promo-note[hidden]", css)
        self.assertIn("display: none !important", rule)
        reduced = css.rsplit("prefers-reduced-motion: reduce", 1)[1]
        self.assertIn(".promo-dismiss", reduced.split("@media", 1)[0])

    def test_the_window_is_declared_once_and_stores_only_locally(self) -> None:
        js = (SITE / "promo.js").read_text(encoding="utf-8")
        self.assertIn('timeZone: "America/Los_Angeles"', js)
        self.assertIn('start: "2026-11-27T00:00:00"', js)
        self.assertIn('end: "2026-12-02T00:00:00"', js)
        self.assertIn('preview: "bf2026"', js)
        self.assertIn("localStorage", js)
        self.assertNotIn("sessionStorage", js)
        self.assertNotIn("document.cookie", js)
        lowered = js.lower()
        for banned in ("googletagmanager", "google-analytics", "gtag(", "fetch("):
            self.assertNotIn(banned, lowered)
        main = (SITE / "main.js").read_text(encoding="utf-8")
        self.assertNotIn("2026-11-27", main)
        self.assertNotIn("bf2026", main)

    def test_claims_match_the_free_episode_not_a_storewide_sale(self) -> None:
        html = (SITE / "index.html").read_text(encoding="utf-8")
        note = html.split('id="bf2026-note"', 1)[1].split("</section>", 1)[0].lower()
        for phrase in (
            "no account",
            "no ads",
            "no telemetry",
            "coming to steam",
            "does not expire",
        ):
            self.assertIn(phrase, note)
        self.assertNotIn("no in-app", note.lower())
        self.assertNotIn("% off", note.lower())

    def test_javascript_window_and_preview_switch(self) -> None:
        completed = subprocess.run(
            ["node", "--test", "tools/site_promo.test.js"],
            cwd=ROOT,
            check=False,
            capture_output=True,
            text=True,
        )
        detail = f"{completed.stdout or ''}\n{completed.stderr or ''}"
        self.assertEqual(completed.returncode, 0, detail)


if __name__ == "__main__":
    unittest.main()
