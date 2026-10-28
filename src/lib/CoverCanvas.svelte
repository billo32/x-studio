<script>
  // Live preview of a cover layout on a W×H screen — the same maths as the Rust bake
  // (crates/x4-books/src/cover.rs). Drag to move the image, scroll or pinch to zoom.
  import { cropSize, fillFor, placeLayout, FULL } from "./geometry.js";

  let { image, crop, fit, zoom, dx, dy, fill, W, H, eink = true, interactive = false, onMove, onZoom } = $props();

  let canvas = $state(null);
  let img = $state(null);
  let dragging = $state(false);

  $effect(() => {
    const el = new Image();
    el.onload = () => (img = el);
    el.src = image.dataUrl;
  });

  $effect(() => {
    if (!canvas || !img) return;
    const k = window.devicePixelRatio || 1;
    canvas.width = Math.round(W * k);
    canvas.height = Math.round(H * k);
    const ctx = canvas.getContext("2d");
    ctx.setTransform(k, 0, 0, k, 0, 0);
    ctx.imageSmoothingQuality = "high";

    // The preview image may be smaller than the original; crop works in fractions.
    const r = crop ?? FULL;
    const sx = r.x * img.naturalWidth, sy = r.y * img.naturalHeight;
    const sw = r.w * img.naturalWidth, sh = r.h * img.naturalHeight;
    const [cw, ch] = cropSize(image, r);
    const g = placeLayout(cw, ch, W, H, { fit, zoom, dx, dy });
    const f = fillFor(fill, image.analysis);

    if (f.kind === "color") {
      ctx.fillStyle = `rgb(${f.color.join(" ")})`;
      ctx.fillRect(0, 0, W, H);
    } else if (f.kind === "blur") {
      const cover = placeLayout(cw, ch, W, H, { fit: "cover" });
      ctx.filter = `blur(${Math.max(1, W / 120) * 4}px)`;
      ctx.drawImage(img, sx, sy, sw, sh, cover.left - 8, cover.top - 8, cover.w + 16, cover.h + 16);
      ctx.filter = "none";
    } else {
      const c = (v) => `rgb(${v.join(" ")})`;
      ctx.fillStyle = c(f.top);
      ctx.fillRect(0, 0, W, Math.max(0, g.top) + 0.5);
      ctx.fillStyle = c(f.bottom);
      ctx.fillRect(0, g.top + g.h - 0.5, W, H);
      ctx.fillStyle = c(f.left);
      ctx.fillRect(0, g.top, Math.max(0, g.left) + 0.5, g.h);
      ctx.fillStyle = c(f.right);
      ctx.fillRect(g.left + g.w - 0.5, g.top, W, g.h);
    }
    ctx.drawImage(img, sx, sy, sw, sh, g.left, g.top, g.w, g.h);
  });

  let last = null;
  function down(e) {
    if (!interactive || e.button !== 0) return;
    dragging = true;
    last = [e.clientX, e.clientY];
    e.currentTarget.setPointerCapture(e.pointerId);
  }
  function move(e) {
    if (!dragging) return;
    const [x, y] = last;
    last = [e.clientX, e.clientY];
    onMove?.((e.clientX - x) / W, (e.clientY - y) / H);
  }
  function up() {
    dragging = false;
  }
  function wheel(e) {
    if (!interactive) return;
    e.preventDefault();
    // Trackpad pinch arrives as a wheel event with ctrlKey; it is finer than a mouse wheel.
    const step = e.ctrlKey ? 0.01 : 0.0015;
    onZoom?.(zoom * Math.exp(-e.deltaY * step));
  }
</script>

<canvas
  bind:this={canvas}
  class:eink
  class:interactive
  class:dragging
  style:width="{W}px"
  style:height="{H}px"
  onpointerdown={down}
  onpointermove={move}
  onpointerup={up}
  onpointercancel={up}
  onwheel={wheel}
  aria-label="Cover preview"
></canvas>

<style>
  canvas { display: block; border-radius: 3px; touch-action: none; }
  .eink { filter: grayscale(1) contrast(1.12); }
  .interactive { cursor: grab; }
  .dragging { cursor: grabbing; }
</style>
