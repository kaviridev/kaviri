/*
 * Recording the playground without asking for a screen.
 *
 * The old "Run and record" asked the browser to capture the tab, which needs a picker every
 * time, does not exist on phones, and in Safari and Firefox often did not exist at all, so the
 * button sat there greyed out. This records every run instead, with no prompt: the app in the
 * iframe is drawn into a canvas (as an SVG foreignObject snapshot, re-taken whenever the app
 * changes), the camera's transform is applied to that drawing every frame, and the canvas
 * stream is what MediaRecorder writes.
 *
 * It is still the preview, not the render: no drawn cursor, no backdrop, no 2x capture and no
 * H.264. Under a phone or window frame it draws the app inside a plain bezel rather than the
 * full device artwork, because the artwork lives in this page's CSS, not the app's.
 */

export function canRecord() {
  const c = document.createElement("canvas");
  return Boolean(window.MediaRecorder && c.captureStream);
}

function pickMime() {
  const wanted = ["video/mp4;codecs=avc1", "video/mp4", "video/webm;codecs=vp9", "video/webm;codecs=vp8", "video/webm"];
  return wanted.find((m) => MediaRecorder.isTypeSupported(m)) || "";
}

/** One SVG image of the app's document as it is right now. */
async function snapshot(doc, w, h) {
  const live = doc.documentElement;
  const clone = live.cloneNode(true);
  clone.querySelectorAll("script").forEach((s) => s.remove());
  // A typed value lives in the property, not the attribute, so a clone would show the
  // placeholder. Copy each field's value across by position.
  const liveFields = live.querySelectorAll("input, textarea, select");
  const cloneFields = clone.querySelectorAll("input, textarea, select");
  liveFields.forEach((el, i) => {
    const c = cloneFields[i];
    if (!c) return;
    if (el.tagName === "TEXTAREA") c.textContent = el.value;
    else if (el.type === "checkbox" || el.type === "radio") el.checked ? c.setAttribute("checked", "") : c.removeAttribute("checked");
    else c.setAttribute("value", el.value);
  });
  // Inside a foreignObject the root is the <svg>, so the app's :root variables would match
  // nothing. They are the same element as <html> in the app, so say that.
  clone.querySelectorAll("style").forEach((s) => {
    s.textContent = s.textContent.replace(/:root\b/g, "html");
  });
  const top = doc.scrollingElement ? doc.scrollingElement.scrollTop : 0;
  clone.setAttribute("style", `${clone.getAttribute("style") || ""};width:${w}px;height:${h}px;overflow:hidden`);
  const body = clone.querySelector("body");
  if (body && top) body.setAttribute("style", `${body.getAttribute("style") || ""};transform:translateY(${-top}px)`);
  const xml = new XMLSerializer().serializeToString(clone);
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}">` +
    `<foreignObject x="0" y="0" width="${w}" height="${h}">${xml}</foreignObject></svg>`;
  const img = new Image();
  await new Promise((resolve, reject) => {
    // A decode that never settles would leave the recorder waiting forever on a stale frame.
    const t = setTimeout(() => reject(new Error("snapshot timed out")), 1000);
    img.onload = () => { clearTimeout(t); resolve(); };
    img.onerror = () => { clearTimeout(t); reject(new Error("snapshot failed to load")); };
    img.src = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(svg);
  });
  return img;
}

/** The element's CSS transform as a DOMMatrix (all camera transforms use origin 0 0). */
function matrixOf(el) {
  const t = getComputedStyle(el).transform;
  return t && t !== "none" ? new DOMMatrix(t) : new DOMMatrix();
}

function roundRect(ctx, x, y, w, h, r) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

/**
 * Start recording. `els` needs frame (the iframe), stage, scene and viewport; `screen` says
 * whether a platform frame is on, in which case the camera moves #scene rather than #stage.
 */
export function startCanvasCapture(els, screen) {
  const frame = els.frame;
  const appW = frame.clientWidth, appH = frame.clientHeight;

  // Geometry is measured once, before the camera moves anything.
  let outW = appW, outH = appH, vp = { x: 0, y: 0, w: appW, h: appH };
  if (screen) {
    const sc = els.scene.getBoundingClientRect();
    const v = els.viewport.getBoundingClientRect();
    outW = Math.round(els.scene.clientWidth);
    outH = Math.round(els.scene.clientHeight);
    vp = { x: v.left - sc.left, y: v.top - sc.top, w: v.width, h: v.height };
  }
  const k = Math.min(2, window.devicePixelRatio || 1);
  const canvas = document.createElement("canvas");
  canvas.width = Math.round(outW * k) & ~1;
  canvas.height = Math.round(outH * k) & ~1;
  const ctx = canvas.getContext("2d");
  const bg = getComputedStyle(screen ? els.scene : frame).backgroundColor || "#fff";

  let img = null, dirty = true, busy = false, stopped = false;
  const mark = () => { dirty = true; };
  const mo = new MutationObserver(mark);
  /*
   * The script's navigate op replaces the iframe's document, so the one present when the
   * capture starts is usually gone a moment later. Watch whichever document is current.
   */
  let doc = null;
  const follow = () => {
    const d = frame.contentDocument;
    if (!d || d === doc || !d.documentElement) return;
    if (doc) {
      doc.removeEventListener("input", mark, true);
      doc.removeEventListener("scroll", mark, true);
    }
    mo.disconnect();
    doc = d;
    mo.observe(doc.documentElement, { subtree: true, childList: true, attributes: true, characterData: true });
    doc.addEventListener("input", mark, true);
    doc.addEventListener("scroll", mark, true);
    dirty = true;
  };
  follow();
  frame.addEventListener("load", follow);

  const refresh = async () => {
    follow();
    if (busy || !dirty || !doc) return;
    busy = true;
    dirty = false;
    try {
      img = await snapshot(doc, appW, appH);
    } catch {
      dirty = true; // try again next frame
      /* keep the last good frame */
    }
    busy = false;
  };

  const draw = () => {
    if (stopped) return;
    refresh();
    ctx.setTransform(k, 0, 0, k, 0, 0);
    ctx.fillStyle = screen ? "#0b0d10" : bg;
    ctx.fillRect(0, 0, outW, outH);
    const m = matrixOf(screen ? els.scene : els.stage);
    ctx.transform(m.a, m.b, m.c, m.d, m.e, m.f);
    if (screen) {
      ctx.fillStyle = "#16181d";
      roundRect(ctx, vp.x - 10, vp.y - 10, vp.w + 20, vp.h + 20, 28);
      ctx.fill();
      ctx.save();
      roundRect(ctx, vp.x, vp.y, vp.w, vp.h, 18);
      ctx.clip();
      if (img) ctx.drawImage(img, vp.x, vp.y, vp.w, vp.h);
      ctx.restore();
    } else if (img) {
      ctx.drawImage(img, 0, 0, appW, appH);
    }
  };
  /*
   * A timer, not requestAnimationFrame: animation frames stop in a background tab and slow
   * to a crawl in a covered window, and either would freeze the take on one frame while the
   * script carried on. 30 a second matches the stream.
   */
  draw();
  const ticker = setInterval(draw, 1000 / 30);

  const stream = canvas.captureStream(30);
  const mimeType = pickMime();
  const rec = new MediaRecorder(stream, mimeType ? { mimeType, videoBitsPerSecond: 6_000_000 } : undefined);
  const chunks = [];
  rec.ondataavailable = (e) => { if (e.data && e.data.size) chunks.push(e.data); };
  rec.start(250);

  return {
    mimeType: rec.mimeType || mimeType || "video/webm",
    abandoned: false,
    async stop() {
      const done = new Promise((resolve) => rec.addEventListener("stop", resolve, { once: true }));
      if (rec.state !== "inactive") rec.stop();
      await done;
      stopped = true;
      clearInterval(ticker);
      mo.disconnect();
      frame.removeEventListener("load", follow);
      if (doc) {
        doc.removeEventListener("input", mark, true);
        doc.removeEventListener("scroll", mark, true);
      }
      stream.getTracks().forEach((t) => t.stop());
      return chunks.length ? new Blob(chunks, { type: this.mimeType }) : null;
    },
  };
}
