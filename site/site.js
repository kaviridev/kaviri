/*
 * The small amount of behaviour the pages share: the header's hairline once the page scrolls,
 * sections that ease in as they arrive, the linked step list and code tabs, the copy button,
 * and just enough syntax colour to read the shape of a code sample.
 *
 * All of it is decoration. With this file blocked every section is visible (the reveal is
 * scoped to html.js, which nav.js adds), the first tab is open and the rest are reachable as
 * hidden panels in the source.
 */

(function () {
  "use strict";

  // Loaded in <head> so the reveal styles apply before the first paint, not after it.
  document.documentElement.classList.add("js");
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init);
  else init();

  function init() {
  // Header hairline
  var head = document.querySelector(".site-head");
  if (head) {
    var onScroll = function () { head.classList.toggle("scrolled", window.scrollY > 4); };
    window.addEventListener("scroll", onScroll, { passive: true });
    onScroll();
  }

  // Reveal on scroll
  var items = document.querySelectorAll(".reveal");
  if ("IntersectionObserver" in window) {
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) {
        if (e.isIntersecting) {
          e.target.classList.add("in");
          io.unobserve(e.target);
        }
      });
    }, { rootMargin: "0px 0px -8% 0px", threshold: 0.08 });
    items.forEach(function (n) { io.observe(n); });
  } else {
    items.forEach(function (n) { n.classList.add("in"); });
  }

  // Tabs. Every [role=tab] inside a [data-tabs] group that controls the same panel moves
  // together, so the step list and the file tabs stay in step.
  document.querySelectorAll("[data-tabs]").forEach(function (group) {
    var tabs = Array.prototype.slice.call(group.querySelectorAll("[role=tab]"));
    function show(id) {
      tabs.forEach(function (t) {
        var on = t.getAttribute("aria-controls") === id;
        t.setAttribute("aria-selected", on ? "true" : "false");
        t.tabIndex = on ? 0 : -1;
      });
      group.querySelectorAll("[role=tabpanel]").forEach(function (p) { p.hidden = p.id !== id; });
    }
    tabs.forEach(function (t) {
      t.addEventListener("click", function () { show(t.getAttribute("aria-controls")); });
      t.addEventListener("keydown", function (e) {
        var list = Array.prototype.slice.call(t.parentNode.querySelectorAll("[role=tab]"));
        var i = list.indexOf(t);
        var next = null;
        if (e.key === "ArrowRight" || e.key === "ArrowDown") next = list[(i + 1) % list.length];
        else if (e.key === "ArrowLeft" || e.key === "ArrowUp") next = list[(i - 1 + list.length) % list.length];
        else if (e.key === "Enter" || e.key === " ") { e.preventDefault(); show(t.getAttribute("aria-controls")); return; }
        if (next) {
          e.preventDefault();
          show(next.getAttribute("aria-controls"));
          next.focus();
        }
      });
    });
  });

  // Copy
  document.querySelectorAll("[data-copy]").forEach(function (b) {
    b.addEventListener("click", function () {
      var src = document.querySelector(b.getAttribute("data-copy"));
      if (!src || !navigator.clipboard) return;
      navigator.clipboard.writeText(src.textContent.trim()).then(function () {
        b.classList.add("done");
        setTimeout(function () { b.classList.remove("done"); }, 1600);
      });
    });
  });

  // Highlighting
  function esc(s) { return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;"); }
  var RULES = {
    json: function (s) {
      return esc(s)
        .replace(/("(?:[^"\\]|\\.)*")(\s*:)/g, '<span class="t-k">$1</span>$2')
        .replace(/(:\s*)("(?:[^"\\]|\\.)*")/g, '$1<span class="t-s">$2</span>')
        .replace(/(:\s*)(-?\d+(?:\.\d+)?)/g, '$1<span class="t-n">$2</span>');
    },
    yaml: function (s) {
      return s.split("\n").map(function (line) {
        var hash = line.search(/\s#\s|^#/);
        var body = hash >= 0 ? line.slice(0, hash) : line;
        var tail = hash >= 0 ? '<span class="t-c">' + esc(line.slice(hash)) + "</span>" : "";
        var m = body.match(/^(\s*-?\s*)([\w.-]+)(:)(.*)$/);
        var out = m ? esc(m[1]) + '<span class="t-k">' + esc(m[2]) + "</span>" + m[3] + '<span class="t-s">' + esc(m[4]) + "</span>" : esc(body);
        return out + tail;
      }).join("\n");
    },
    sh: function (s) {
      return s.split("\n").map(function (line) {
        if (/^\s*#/.test(line)) return '<span class="t-c">' + esc(line) + "</span>";
        return esc(line).replace(/^(\w[\w-]*)/, '<span class="t-k">$1</span>').replace(/(\s)(--?[\w-]+)/g, '$1<span class="t-n">$2</span>');
      }).join("\n");
    }
  };
  document.querySelectorAll("pre[data-lang]").forEach(function (pre) {
    var rule = RULES[pre.getAttribute("data-lang")];
    var code = pre.querySelector("code") || pre;
    if (rule) code.innerHTML = rule(code.textContent);
  });
  }
})();
