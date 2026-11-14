/*
 * The video player.
 *
 * Every <video data-player> on the site is upgraded into the same small player: one big play
 * button, a bar with play, a scrubber, the time, speed and fullscreen, and keyboard control.
 * The browser's own controls look different in every browser and, for a WebM that came out of
 * MediaRecorder, cannot seek at all, because such a file has no duration in its header. The
 * player measures it (see fixDuration) so the scrubber works on the playground's preview too.
 *
 * Without this file the <video> keeps whatever the markup gave it, so the hero still loops and
 * the playground's preview falls back to native controls (play.js sets them if this is absent).
 *
 * Options, as attributes on the <video>:
 *   data-player              upgrade this video
 *   data-player-ambient      a looping muted hero: controls stay out of the way until hovered,
 *                            and it pauses while scrolled off screen
 *   data-player-audio        show a mute button (a silent demo has nothing to mute)
 */

(function () {
  "use strict";

  var SPEEDS = [1, 1.5, 2, 0.5];

  var ICON = {
    play: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5.5v13a1 1 0 0 0 1.5.86l10.4-6.5a1 1 0 0 0 0-1.72L9.5 4.64A1 1 0 0 0 8 5.5z"/></svg>',
    pause: '<svg viewBox="0 0 24 24" aria-hidden="true"><rect x="6.5" y="5" width="4" height="14" rx="1.2"/><rect x="13.5" y="5" width="4" height="14" rx="1.2"/></svg>',
    replay: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 5V2L7.5 6 12 10V7a5 5 0 1 1-5 5H5a7 7 0 1 0 7-7z"/></svg>',
    full: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 9V5a1 1 0 0 1 1-1h4v2H6v3H4zm11-5h4a1 1 0 0 1 1 1v4h-2V6h-3V4zM4 15h2v3h3v2H5a1 1 0 0 1-1-1v-4zm14 0h2v4a1 1 0 0 1-1 1h-4v-2h3v-3z"/></svg>',
    exit: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M9 4v4a1 1 0 0 1-1 1H4V7h3V4h2zm6 0h2v3h3v2h-4a1 1 0 0 1-1-1V4zM4 15h4a1 1 0 0 1 1 1v4H7v-3H4v-2zm11 1a1 1 0 0 1 1-1h4v2h-3v3h-2v-4z"/></svg>',
    sound: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 9h3.5L12 5v14l-4.5-4H4a1 1 0 0 1-1-1v-4a1 1 0 0 1 1-1zm11.5-1.5a6 6 0 0 1 0 9l-1.4-1.4a4 4 0 0 0 0-6.2l1.4-1.4z"/></svg>',
    muted: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 9h3.5L12 5v14l-4.5-4H4a1 1 0 0 1-1-1v-4a1 1 0 0 1 1-1zm11.3.3 1.4-1.4L19 10.2l2.3-2.3 1.4 1.4-2.3 2.3 2.3 2.3-1.4 1.4-2.3-2.3-2.3 2.3-1.4-1.4 2.3-2.3-2.3-2.3z"/></svg>'
  };

  function fmt(s) {
    if (!isFinite(s) || s < 0) s = 0;
    var m = Math.floor(s / 60);
    var r = Math.floor(s % 60);
    return m + ":" + (r < 10 ? "0" : "") + r;
  }

  function el(tag, cls, html) {
    var n = document.createElement(tag);
    if (cls) n.className = cls;
    if (html) n.innerHTML = html;
    return n;
  }

  /*
   * A WebM from MediaRecorder reports a duration of Infinity until the browser has read to the
   * end of it. Seeking far past the end makes it do exactly that, after which durationchange
   * carries the real length and the playhead goes back to the start.
   */
  function fixDuration(v, done) {
    if (isFinite(v.duration)) return done();
    var wasMuted = v.muted;
    v.muted = true;
    var settle = function () {
      if (!isFinite(v.duration)) return;
      v.removeEventListener("durationchange", settle);
      v.currentTime = 0;
      v.muted = wasMuted;
      done();
    };
    v.addEventListener("durationchange", settle);
    try { v.currentTime = 1e101; } catch (e) { done(); }
  }

  function upgrade(v) {
    if (v.dataset.playerReady) return;
    v.dataset.playerReady = "1";

    var ambient = v.hasAttribute("data-player-ambient");
    var audio = v.hasAttribute("data-player-audio");
    var reduce = window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches;

    v.controls = false;
    v.removeAttribute("controls");
    if (ambient && reduce) {
      v.autoplay = false;
      v.removeAttribute("autoplay");
    }

    var box = el("div", "kp" + (ambient ? " kp-ambient" : ""));
    box.tabIndex = 0;
    box.setAttribute("role", "group");
    box.setAttribute("aria-label", v.getAttribute("aria-label") || "Video player");
    v.parentNode.insertBefore(box, v);
    box.appendChild(v);
    v.removeAttribute("aria-label");
    v.setAttribute("aria-hidden", "true");

    var big = el("button", "kp-big", ICON.play);
    big.type = "button";
    big.setAttribute("aria-label", "Play");

    var bar = el("div", "kp-bar");
    var playBtn = el("button", "kp-btn", ICON.play);
    playBtn.type = "button";
    playBtn.setAttribute("aria-label", "Play");

    var track = el("div", "kp-track");
    track.tabIndex = 0;
    track.setAttribute("role", "slider");
    track.setAttribute("aria-label", "Seek");
    track.setAttribute("aria-valuemin", "0");
    var buf = el("div", "kp-buf");
    var fill = el("div", "kp-fill");
    var knob = el("div", "kp-knob");
    var hoverTip = el("div", "kp-tip");
    track.append(buf, fill, knob, hoverTip);

    var time = el("span", "kp-time", "0:00 / 0:00");

    var speedBtn = el("button", "kp-btn kp-speed", "1x");
    speedBtn.type = "button";
    speedBtn.setAttribute("aria-label", "Playback speed");

    var muteBtn = null;
    if (audio) {
      muteBtn = el("button", "kp-btn", v.muted ? ICON.muted : ICON.sound);
      muteBtn.type = "button";
      muteBtn.setAttribute("aria-label", v.muted ? "Unmute" : "Mute");
    }

    var fullBtn = el("button", "kp-btn", ICON.full);
    fullBtn.type = "button";
    fullBtn.setAttribute("aria-label", "Fullscreen");

    bar.append(playBtn, track, time, speedBtn);
    if (muteBtn) bar.append(muteBtn);
    if (box.requestFullscreen || v.webkitEnterFullscreen) bar.append(fullBtn);
    box.append(big, bar);

    var duration = 0;
    var raf = 0;
    var dragging = false;
    var idle = 0;
    var userPaused = false;

    function setDuration() {
      duration = isFinite(v.duration) ? v.duration : 0;
      track.setAttribute("aria-valuemax", String(Math.round(duration)));
      paint();
    }

    function paint() {
      var t = v.currentTime || 0;
      var p = duration ? Math.min(1, t / duration) : 0;
      fill.style.transform = "scaleX(" + p + ")";
      knob.style.left = p * 100 + "%";
      time.textContent = fmt(t) + " / " + fmt(duration);
      track.setAttribute("aria-valuenow", String(Math.round(t)));
      track.setAttribute("aria-valuetext", fmt(t) + " of " + fmt(duration));
      if (v.buffered.length && duration) {
        buf.style.transform = "scaleX(" + Math.min(1, v.buffered.end(v.buffered.length - 1) / duration) + ")";
      }
    }

    function loop() {
      paint();
      raf = requestAnimationFrame(loop);
    }

    function sync() {
      var playing = !v.paused && !v.ended;
      box.classList.toggle("is-playing", playing);
      box.classList.toggle("is-ended", v.ended);
      var icon = v.ended ? ICON.replay : playing ? ICON.pause : ICON.play;
      var label = playing ? "Pause" : "Play";
      playBtn.innerHTML = icon;
      playBtn.setAttribute("aria-label", label);
      big.innerHTML = v.ended ? ICON.replay : ICON.play;
      big.setAttribute("aria-label", label);
      cancelAnimationFrame(raf);
      if (playing) raf = requestAnimationFrame(loop);
      else paint();
    }

    function toggle() {
      if (v.paused || v.ended) {
        userPaused = false;
        var p = v.play();
        if (p && p.catch) p.catch(function () {});
      } else {
        userPaused = true;
        v.pause();
      }
      wake();
    }

    function seekTo(t) {
      if (!duration) return;
      v.currentTime = Math.max(0, Math.min(duration - 0.05, t));
      paint();
    }

    function ratioAt(e) {
      var r = track.getBoundingClientRect();
      return Math.max(0, Math.min(1, (e.clientX - r.left) / r.width));
    }

    function wake() {
      box.classList.add("is-awake");
      clearTimeout(idle);
      idle = setTimeout(function () {
        if (!v.paused && !dragging && !box.contains(document.activeElement)) box.classList.remove("is-awake");
      }, 2200);
    }

    // Wiring

    big.addEventListener("click", toggle);
    playBtn.addEventListener("click", toggle);
    v.addEventListener("click", toggle);

    speedBtn.addEventListener("click", function () {
      var i = (SPEEDS.indexOf(v.playbackRate) + 1) % SPEEDS.length;
      v.playbackRate = SPEEDS[i];
      speedBtn.textContent = SPEEDS[i] + "x";
    });

    if (muteBtn) {
      muteBtn.addEventListener("click", function () {
        v.muted = !v.muted;
      });
      v.addEventListener("volumechange", function () {
        muteBtn.innerHTML = v.muted ? ICON.muted : ICON.sound;
        muteBtn.setAttribute("aria-label", v.muted ? "Unmute" : "Mute");
      });
    }

    fullBtn.addEventListener("click", function () {
      if (document.fullscreenElement) document.exitFullscreen();
      else if (box.requestFullscreen) box.requestFullscreen();
      else if (v.webkitEnterFullscreen) v.webkitEnterFullscreen();
    });
    document.addEventListener("fullscreenchange", function () {
      var on = document.fullscreenElement === box;
      box.classList.toggle("is-full", on);
      fullBtn.innerHTML = on ? ICON.exit : ICON.full;
      fullBtn.setAttribute("aria-label", on ? "Exit fullscreen" : "Fullscreen");
    });

    track.addEventListener("pointerdown", function (e) {
      dragging = true;
      box.classList.add("is-dragging");
      track.setPointerCapture(e.pointerId);
      seekTo(ratioAt(e) * duration);
    });
    track.addEventListener("pointermove", function (e) {
      var r = ratioAt(e);
      hoverTip.textContent = fmt(r * duration);
      hoverTip.style.left = r * 100 + "%";
      if (dragging) seekTo(r * duration);
    });
    var endDrag = function () {
      dragging = false;
      box.classList.remove("is-dragging");
      wake();
    };
    track.addEventListener("pointerup", endDrag);
    track.addEventListener("pointercancel", endDrag);

    box.addEventListener("keydown", function (e) {
      if (e.target.closest("button") && (e.key === " " || e.key === "Enter")) return;
      var k = e.key.toLowerCase();
      if (k === " " || k === "k") toggle();
      else if (k === "arrowleft") seekTo(v.currentTime - 5);
      else if (k === "arrowright") seekTo(v.currentTime + 5);
      else if (k === "home") seekTo(0);
      else if (k === "end") seekTo(duration);
      else if (k === "f") fullBtn.click();
      else if (k === "m" && muteBtn) muteBtn.click();
      else return;
      e.preventDefault();
      wake();
    });

    box.addEventListener("pointermove", wake);
    box.addEventListener("focusin", wake);
    box.addEventListener("pointerleave", function () {
      if (!v.paused) box.classList.remove("is-awake");
    });

    v.addEventListener("loadedmetadata", function () {
      fixDuration(v, setDuration);
    });
    v.addEventListener("durationchange", setDuration);
    ["play", "playing", "pause", "ended", "seeked"].forEach(function (ev) {
      v.addEventListener(ev, sync);
    });
    v.addEventListener("progress", paint);
    v.addEventListener("waiting", function () { box.classList.add("is-waiting"); });
    v.addEventListener("playing", function () { box.classList.remove("is-waiting"); });
    v.addEventListener("emptied", function () {
      duration = 0;
      v.playbackRate = 1;
      speedBtn.textContent = "1x";
      paint();
      sync();
    });

    // An ambient hero only plays while someone can see it, and resumes unless they paused it.
    if (ambient && "IntersectionObserver" in window && !reduce) {
      new IntersectionObserver(function (entries) {
        entries.forEach(function (en) {
          if (en.isIntersecting) {
            if (!userPaused && v.paused) {
              var p = v.play();
              if (p && p.catch) p.catch(function () {});
            }
          } else if (!v.paused) {
            v.pause();
          }
        });
      }, { threshold: 0.2 }).observe(box);
    }

    if (v.readyState >= 1) fixDuration(v, setDuration);
    sync();
  }

  function all() {
    document.querySelectorAll("video[data-player]").forEach(upgrade);
  }

  window.kaviriPlayer = upgrade;
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", all);
  else all();
})();
