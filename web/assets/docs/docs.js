import "phoenix_html";
import { Socket } from "phoenix";
import { LiveSocket } from "phoenix_live_view";
import topbar from "../vendor/topbar.js";
import Noora from "noora";
import DocsContentHook from "./hooks/docs-content-hook.js";
import DocsCopyPageButtonHook, { dispatchCopyPageButtonFlash } from "./hooks/docs-copy-page-button-hook.js";
import { copyTextToClipboard } from "./shared/clipboard.js";

import "./docs.css";

const csrfToken = document.querySelector("meta[name='csrf-token']").getAttribute("content");

const liveSocket = new LiveSocket("/live", Socket, {
  longPollFallbackMs: 2500,
  params: { _csrf_token: csrfToken },
  hooks: {
    ...Noora.Hooks,
    DocsContent: DocsContentHook,
    DocsCopyPageButton: DocsCopyPageButtonHook,
  },
});

topbar.config({ barColors: { 0: "#7c3aed" }, shadowColor: "rgba(0, 0, 0, .3)" });

function closeMobileSidebar() {
  document.body.removeAttribute("data-sidebar-open");
  document.getElementById("docs-sidebar")?.removeAttribute("data-mobile-open");
}

window.addEventListener("phx:page-loading-start", () => topbar.show(300));
window.addEventListener("phx:page-loading-stop", (info) => {
  topbar.hide();
  closeMobileSidebar();
  const to = info.detail?.to;
  if (to && info.detail?.kind !== "initial") {
    const destination = new URL(to, window.location.origin);
    if (!destination.hash) requestAnimationFrame(() => window.scrollTo(0, 0));
  }
});

liveSocket.connect();
window.liveSocket = liveSocket;

window.addEventListener("phx:docs:copy-to-clipboard", ({ detail }) => {
  copyTextToClipboard(detail.text)
    .then(() => dispatchCopyPageButtonFlash())
    .catch((error) => console.error("Failed to copy page:", error));
});

// Table-of-contents scroll spy: highlight the heading currently in view.
function setupTocScrollSpy() {
  const toc = document.getElementById("docs-toc");
  if (!toc) return null;

  const tocLinks = toc.querySelectorAll('[data-part="list"] a');
  if (!tocLinks.length) return null;

  const headings = Array.from(tocLinks)
    .map((a) => document.getElementById(a.getAttribute("href")?.replace("#", "")))
    .filter(Boolean);
  if (!headings.length) return null;

  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (entry.isIntersecting) {
          tocLinks.forEach((link) => link.removeAttribute("data-active"));
          toc.querySelector(`[data-part="list"] a[href="#${entry.target.id}"]`)?.setAttribute("data-active", "");
          break;
        }
      }
    },
    { rootMargin: "0px 0px -80% 0px", threshold: 0 },
  );

  headings.forEach((h) => observer.observe(h));
  return observer;
}

let tocObserver = null;
window.addEventListener("phx:page-loading-stop", () => {
  tocObserver?.disconnect();
  requestAnimationFrame(() => {
    tocObserver = setupTocScrollSpy();
  });
});
requestAnimationFrame(() => {
  tocObserver = setupTocScrollSpy();
});
