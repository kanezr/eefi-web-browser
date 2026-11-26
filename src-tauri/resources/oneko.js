(() => {
  if (window.top !== window || window.__eefiOnekoBooted) return;
  window.__eefiOnekoBooted = true;

  const spriteData = __EEFI_ONEKO_SPRITES__;
  const surfaceId = "__eefi_oneko_surface_7c9a2d";
  const canvasId = "__eefi_oneko_canvas_7c9a2d";
  const settings = {
    enabled: false,
    speed: "normal",
    size: 1,
    showBrowserPages: false,
  };
  let hideOnSite = false;
  let updateRevision = 0;
  let generation = 0;
  let active = false;
  let reducedMotion = false;
  let motionQuery;
  let frames;
  let framePromise;
  let documentRootObserver;
  let documentRootResolver;
  let surface;
  let shadow;
  let canvas;
  let context;
  let positionRule;
  let animationFrame = 0;
  let lastFrameTime = 0;
  let tickAccumulator = 0;
  let width = 32;
  let height = 32;
  let x = 0;
  let y = 0;
  let targetX = 0;
  let targetY = 0;
  let state = 0;
  let imageNumber = 0;
  let init = 0;
  let sleep = 0;
  let imageLoop1 = 0;
  let imageLoop2 = 0;
  let mouseMoved = false;

  function closeFrames(images) {
    if (images) {
      for (const image of images) image.close();
    }
  }

  async function loadFrames(runGeneration) {
    if (frames) return frames;
    const pending = (async () => {
      const loaded = [];
      try {
        for (const dataUrl of spriteData) {
          const encoded = dataUrl.slice(dataUrl.indexOf(",") + 1);
          const binary = atob(encoded);
          const bytes = new Uint8Array(binary.length);
          for (let index = 0; index < binary.length; index += 1) {
            bytes[index] = binary.charCodeAt(index);
          }
          loaded.push(await createImageBitmap(new Blob([bytes], { type: "image/gif" })));
        }
        return loaded;
      } catch (error) {
        closeFrames(loaded);
        throw error;
      }
    })();
    framePromise = pending;
    try {
      const loaded = await pending;
      if (runGeneration !== generation || !active) {
        closeFrames(loaded);
        return null;
      }
      frames = loaded;
      width = loaded[0].width;
      height = loaded[0].height;
      return frames;
    } finally {
      if (framePromise === pending) framePromise = undefined;
    }
  }

  function createSurface() {
    surface = document.createElement("div");
    surface.id = surfaceId;
    surface.setAttribute("aria-hidden", "true");
    shadow = surface.attachShadow({ mode: "closed" });
    canvas = document.createElement("canvas");
    canvas.id = canvasId;
    context = canvas.getContext("2d", { alpha: true });
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(
      ":host{all:initial!important;position:fixed!important;left:0!important;top:0!important;width:0!important;height:0!important;z-index:2147483647!important;pointer-events:none!important}" +
        "canvas{position:fixed!important;left:0;top:0;width:0;height:0;display:block!important;pointer-events:none!important}",
    );
    shadow.adoptedStyleSheets = [sheet];
    positionRule = sheet.cssRules[1];
    shadow.append(canvas);
    document.documentElement.append(surface);
  }

  function waitForDocumentRoot(runGeneration) {
    if (document.documentElement) return Promise.resolve(true);
    return new Promise((resolve) => {
      documentRootResolver = resolve;
      documentRootObserver = new MutationObserver(() => {
        if (!document.documentElement) return;
        documentRootObserver.disconnect();
        documentRootObserver = undefined;
        documentRootResolver = undefined;
        resolve(runGeneration === generation && active);
      });
      documentRootObserver.observe(document, { childList: true });
    });
  }

  function draw(number) {
    if (!canvas || !context || !frames) return;
    const scale = settings.size === 2 ? 2 : 1;
    const pixelWidth = width * scale;
    const pixelHeight = height * scale;
    canvas.width = pixelWidth;
    canvas.height = pixelHeight;
    positionRule.style.setProperty("width", `${pixelWidth}px`);
    positionRule.style.setProperty("height", `${pixelHeight}px`);
    positionRule.style.setProperty("left", `${x - pixelWidth / 2}px`);
    positionRule.style.setProperty("top", `${y - pixelHeight * (30 / 32)}px`);
    const frameIndex = number === 0 ? 24 : number - 1;
    context.clearRect(0, 0, pixelWidth, pixelHeight);
    context.drawImage(frames[frameIndex], 0, 0, pixelWidth, pixelHeight);
  }

  function usableBounds() {
    const scale = settings.size === 2 ? 2 : 1;
    const catWidth = width * scale;
    const catHeight = height * scale;
    return {
      left: catWidth / 2,
      top: catHeight,
      right: Math.max(catWidth / 2, window.innerWidth - catWidth / 2),
      bottom: Math.max(catHeight, window.innerHeight - catHeight),
    };
  }

  function frameDelay(framesPerSecond) {
    return Math.floor(1000 / framesPerSecond);
  }

  function animate(position, theta, distance) {
    if (state === 0) {
      if (init < 33) {
        sleep = 50;
        const bounds = usableBounds();
        x = bounds.left + (bounds.right - bounds.left) / 2;
        y = bounds.top + (bounds.bottom - bounds.top) / 2;
        imageNumber = init;
        init += 1;
      } else {
        state = 1;
      }
    } else if (state === 1) {
      sleep = frameDelay(8);
      let run = 16;
      if (settings.speed === "slow") run *= 0.5;
      if (settings.speed === "fast") run *= 1.5;
      run = Math.min(run, distance);
      x = Math.trunc(x + Math.cos(theta) * run);
      y = Math.trunc(y - Math.sin(theta) * run);
      distance -= run;
      if (distance < 2) state = 2;

      if (theta >= -Math.PI / 8 && theta <= Math.PI / 8) {
        imageNumber = imageNumber === 5 ? 6 : 5;
      }
      if (theta > Math.PI / 8 && theta < (3 * Math.PI) / 8) {
        imageNumber = imageNumber === 3 ? 4 : 3;
      }
      if (theta >= (3 * Math.PI) / 8 && theta <= (5 * Math.PI) / 8) {
        imageNumber = imageNumber === 1 ? 2 : 1;
      }
      if (theta > (5 * Math.PI) / 8 && theta < (7 * Math.PI) / 8) {
        imageNumber = imageNumber === 15 ? 16 : 15;
      }
      if (theta >= (7 * Math.PI) / 8 || theta <= (-7 * Math.PI) / 8) {
        imageNumber = imageNumber === 13 ? 14 : 13;
      }
      if (theta > (-7 * Math.PI) / 8 && theta < (-5 * Math.PI) / 8) {
        imageNumber = imageNumber === 11 ? 12 : 11;
      }
      if (theta >= (-5 * Math.PI) / 8 && theta <= (-3 * Math.PI) / 8) {
        imageNumber = imageNumber === 9 ? 10 : 9;
      }
      if (theta > (-3 * Math.PI) / 8 && theta < -Math.PI / 8) {
        imageNumber = imageNumber === 7 ? 8 : 7;
      }
      mouseMoved = false;
    } else {
      switch (imageNumber) {
        case 0:
          if (position) {
            sleep = frameDelay(3);
            imageNumber =
              position === "over"
                ? 17
                : position === "under"
                  ? 21
                  : position === "left"
                    ? 23
                    : 19;
            break;
          }
          sleep = frameDelay(3);
          imageNumber = 31;
          break;
        case 17:
          sleep = frameDelay(3);
          imageNumber = 18;
          imageLoop1 += 1;
          if (imageLoop1 === 6) {
            imageNumber = 27;
            imageLoop1 = 0;
          }
          break;
        case 18:
          sleep = frameDelay(3);
          imageNumber = 17;
          break;
        case 21:
          sleep = frameDelay(3);
          imageNumber = 22;
          imageLoop1 += 1;
          if (imageLoop1 === 6) {
            imageNumber = 27;
            imageLoop1 = 0;
          }
          break;
        case 22:
          sleep = frameDelay(3);
          imageNumber = 21;
          break;
        case 23:
          sleep = frameDelay(3);
          imageNumber = 24;
          imageLoop1 += 1;
          if (imageLoop1 === 6) {
            imageNumber = 27;
            imageLoop1 = 0;
          }
          break;
        case 24:
          sleep = frameDelay(3);
          imageNumber = 23;
          break;
        case 19:
          sleep = frameDelay(3);
          imageNumber = 20;
          imageLoop1 += 1;
          if (imageLoop1 === 6) {
            imageNumber = 27;
            imageLoop1 = 0;
          }
          break;
        case 20:
          sleep = frameDelay(3);
          imageNumber = 19;
          break;
        case 31:
          sleep = frameDelay(3);
          imageNumber = 25;
          imageLoop1 += 1;
          if (imageLoop1 === 6) {
            sleep = frameDelay(10);
            imageNumber = 27;
            imageLoop1 = 0;
          }
          break;
        case 25:
          sleep = frameDelay(3);
          imageNumber = 31;
          break;
        case 27:
          sleep = frameDelay(10);
          imageNumber = 28;
          break;
        case 28:
          imageNumber = 27;
          imageLoop2 += 1;
          if (imageLoop2 === 4) {
            imageNumber = 26;
            sleep = 1500;
            imageLoop2 = 0;
          }
          break;
        case 26:
          imageNumber = 29;
          sleep = 1200;
          break;
        case 29:
          imageNumber = 30;
          sleep = 1200;
          break;
        case 30:
          imageNumber = 29;
          sleep = 1200;
          break;
        default:
          imageNumber = 0;
          break;
      }
      if (mouseMoved) {
        sleep = 1000;
        imageNumber = 32;
        imageLoop1 = 0;
        imageLoop2 = 0;
        state = 1;
      }
    }
    draw(imageNumber);
  }

  function tick() {
    const bounds = usableBounds();
    let position = null;
    let goalX = targetX;
    let goalY = targetY;
    if (goalY < bounds.top) {
      goalY = bounds.top;
      position = "over";
    }
    if (goalY > bounds.bottom) {
      goalY = bounds.bottom;
      position = "under";
    }
    if (goalX < bounds.left) {
      goalX = bounds.left;
      position = "left";
    }
    if (goalX > bounds.right) {
      goalX = bounds.right;
      position = "right";
    }
    const dx = goalX - x;
    const dy = y - goalY;
    const distance = Math.sqrt(dx * dx + dy * dy);
    mouseMoved = mouseMoved || distance > 64;
    sleep = Math.max(0, sleep - 50);
    if (sleep === 0) animate(position, Math.atan2(dy, dx), distance);
  }

  function onPointerMove(event) {
    targetX = event.clientX - 2;
    targetY = event.clientY - 3;
  }

  function runAnimationFrame(time) {
    if (!active || reducedMotion) return;
    if (!lastFrameTime) lastFrameTime = time;
    tickAccumulator = Math.min(250, tickAccumulator + time - lastFrameTime);
    lastFrameTime = time;
    while (tickAccumulator >= 50 && active && !reducedMotion) {
      tickAccumulator -= 50;
      tick();
    }
    animationFrame = requestAnimationFrame(runAnimationFrame);
  }

  function startAnimation() {
    targetX = window.innerWidth / 2 - 2;
    targetY = window.innerHeight / 2 - 3;
    x = window.innerWidth / 2;
    y = window.innerHeight / 2;
    state = 0;
    imageNumber = 0;
    init = 0;
    sleep = 0;
    imageLoop1 = 0;
    imageLoop2 = 0;
    mouseMoved = false;
    lastFrameTime = 0;
    tickAccumulator = 0;
    window.addEventListener("pointermove", onPointerMove, { passive: true });
    animationFrame = requestAnimationFrame(runAnimationFrame);
  }

  function renderReducedMotion() {
    const bounds = usableBounds();
    x = bounds.left + (bounds.right - bounds.left) / 2;
    y = bounds.top + (bounds.bottom - bounds.top) / 2;
    draw(25);
  }

  function stopRuntime() {
    generation += 1;
    active = false;
    if (documentRootObserver) documentRootObserver.disconnect();
    documentRootObserver = undefined;
    if (documentRootResolver) documentRootResolver(false);
    documentRootResolver = undefined;
    if (animationFrame) cancelAnimationFrame(animationFrame);
    animationFrame = 0;
    window.removeEventListener("pointermove", onPointerMove);
    if (motionQuery) motionQuery.removeEventListener("change", onMotionChange);
    motionQuery = undefined;
    if (surface) surface.remove();
    surface = undefined;
    shadow = undefined;
    canvas = undefined;
    context = undefined;
    positionRule = undefined;
    closeFrames(frames);
    frames = undefined;
    framePromise = undefined;
  }

  function onMotionChange() {
    if (!active) return;
    const wasReduced = reducedMotion;
    reducedMotion = motionQuery.matches;
    if (wasReduced === reducedMotion || !frames) return;
    window.removeEventListener("pointermove", onPointerMove);
    if (animationFrame) cancelAnimationFrame(animationFrame);
    animationFrame = 0;
    if (reducedMotion) {
      renderReducedMotion();
    } else {
      startAnimation();
    }
  }

  async function startRuntime() {
    const runGeneration = ++generation;
    active = true;
    if (!(await waitForDocumentRoot(runGeneration))) return;
    createSurface();
    reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    motionQuery = window.matchMedia("(prefers-reduced-motion: reduce)");
    motionQuery.addEventListener("change", onMotionChange);
    const loaded = await loadFrames(runGeneration);
    if (!loaded || runGeneration !== generation || !active) return;
    if (reducedMotion) {
      renderReducedMotion();
    } else {
      startAnimation();
    }
  }

  function isBrowserPage() {
    return (
      window.__eefiOnekoBrowserUi === true ||
      (location.protocol !== "http:" && location.protocol !== "https:")
    );
  }

  function reconcile() {
    const shouldRun =
      settings.enabled &&
      !hideOnSite &&
      (!isBrowserPage() || settings.showBrowserPages);
    if (!shouldRun) {
      if (active || surface || frames || framePromise) stopRuntime();
      return;
    }
    if (!active) {
      void startRuntime().catch((error) => {
        console.error("Eefi Browser: could not start the Oneko cat.", error);
        stopRuntime();
      });
      return;
    }
    if (reducedMotion) {
      renderReducedMotion();
    } else {
      draw(imageNumber);
    }
  }

  function update(incoming) {
    if (!incoming || typeof incoming !== "object") return;
    updateRevision += 1;
    if (
      typeof incoming.siteHost === "string" &&
      incoming.siteHost.toLowerCase() === location.hostname.toLowerCase()
    ) {
      hideOnSite = Boolean(incoming.hideOnSite);
    }
    if (typeof incoming.enabled === "boolean") settings.enabled = incoming.enabled;
    if (["slow", "normal", "fast"].includes(incoming.speed)) {
      settings.speed = incoming.speed;
    }
    if (incoming.size === 1 || incoming.size === 2) settings.size = incoming.size;
    if (typeof incoming.showBrowserPages === "boolean") {
      settings.showBrowserPages = incoming.showBrowserPages;
    }
    reconcile();
  }

  Object.defineProperty(window, "__eefiOnekoUpdate", {
    configurable: true,
    value: update,
  });

  const internals = window.__TAURI_INTERNALS__;
  if (
    internals &&
    typeof internals.invoke === "function" &&
    (location.protocol === "http:" || location.protocol === "https:")
  ) {
    const initialRevision = updateRevision;
    internals
      .invoke("oneko_settings_get", { host: location.hostname })
      .then((initialSettings) => {
        if (initialRevision === updateRevision) update(initialSettings);
      })
      .catch((error) => {
        console.error("Eefi Browser: could not read Oneko settings.", error);
      });
  }
})();
