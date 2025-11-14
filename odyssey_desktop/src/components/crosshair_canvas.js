(function () {
  if (window.__odysseyCrosshair) {
    return;
  }

  const state = {
    canvas: null,
    ctx: null,
    points: {},
    images: new Map(),
  };

  function ensureCanvas() {
    if (state.canvas && state.ctx) {
      return true;
    }
    state.canvas = document.getElementById("crosshair-layer");
    if (!state.canvas) {
      state.ctx = null;
      return false;
    }
    state.ctx = state.canvas.getContext("2d");
    return true;
  }

  function getImage(src) {
    let entry = state.images.get(src);
    if (!entry) {
      const img = new Image();
      entry = { img, ready: false };
      img.onload = () => {
        entry.ready = true;
        render();
      };
      img.onerror = () => {
        console.warn("Failed to load crosshair image", src);
      };
      img.src = src;
      state.images.set(src, entry);
    }
    return entry;
  }

  function render() {
    if (!ensureCanvas()) {
      return;
    }
    const { canvas, ctx } = state;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    Object.values(state.points).forEach((point) => {
      const entry = getImage(point.src);
      if (!entry.ready || !entry.img.complete) {
        return;
      }
      const w = entry.img.naturalWidth;
      const h = entry.img.naturalHeight;
      ctx.drawImage(entry.img, point.x - w / 2, point.y - h / 2);
    });
  }

  window.__odysseyCrosshair = {
    resize(width, height) {
      if (!ensureCanvas()) {
        return;
      }
      const canvas = document.getElementById("crosshair-layer");
      if (!canvas) {
        return;
      }
      const dpr = window.devicePixelRatio || 1;
      canvas.width = width * dpr;
      canvas.height = height * dpr;
      canvas.style.width = `${width}px`;
      canvas.style.height = `${height}px`;
      const ctx = canvas.getContext("2d");
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.scale(dpr, dpr);
      state.canvas = canvas;
      state.ctx = ctx;
      render();
    },
    draw(key, x, y, src) {
      if (!ensureCanvas()) {
        return;
      }
      state.points[key] = { x, y, src };
      render();
    },
    clearKey(key) {
      delete state.points[key];
      render();
    },
  };
})();
