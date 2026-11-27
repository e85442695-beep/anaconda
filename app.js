(() => {
  const PRESETS = {
    native: `# 1. Clone & build Anaconda
git clone https://github.com/e85442695-beep/anaconda.git
cd anaconda
cargo build --release --locked -p victory

# 2. Launch with pure native FPS + HUD (F4 toggle)
$env:VICTORY_GAME_DIR  = "D:\\Path\\To\\YourDump"
$env:VICTORY_FRAME_GEN = "0"
$env:VICTORY_FPS_HUD   = "right"
.\\Play-Anaconda.cmd`,
    fg: `# 1. Build Anaconda (once)
cargo build --release --locked -p victory

# 2. Launch with 2x HUD-less Optical Flow FrameGen
$env:VICTORY_GAME_DIR  = "D:\\Path\\To\\YourDump"
$env:VICTORY_FRAME_GEN = "1"
$env:VICTORY_FSR_SCALE = "1.5"
$env:VICTORY_FPS_HUD   = "right"
.\\Play-Anaconda.cmd`,
    ultralow: `# Strict 4 GB VRAM ceiling + aggressive FSR Performance
$env:VICTORY_GAME_DIR          = "D:\\Path\\To\\YourDump"
$env:VICTORY_VRAM_MB           = "2950"
$env:VICTORY_TEXTURE_BUDGET_MB = "750"
$env:VICTORY_TILES_HEAP_MB     = "16"
$env:VICTORY_FSR_SCALE         = "1.7"
$env:VICTORY_FRAME_GEN         = "1"
.\\Play-Anaconda.cmd`
  };

  let currentPreset = "native";

  const video = document.getElementById("demo-video");
  const chapterButtons = Array.from(document.querySelectorAll(".chapter-btn"));
  const presetTabs = Array.from(document.querySelectorAll(".preset-tab"));
  const snippetEl = document.getElementById("preset-code");
  const copyBtn = document.getElementById("copy-code-btn");

  function applyPreset(name) {
    currentPreset = name;
    if (snippetEl && PRESETS[name]) {
      snippetEl.textContent = PRESETS[name];
    }
    presetTabs.forEach((tab) => {
      const active = tab.getAttribute("data-preset") === name;
      tab.classList.toggle("is-active", active);
      tab.setAttribute("aria-selected", String(active));
    });
  }

  function updateActiveChapter(currentTime) {
    let activeBtn = chapterButtons[0];
    for (const btn of chapterButtons) {
      const t = Number(btn.getAttribute("data-time") || "0");
      if (currentTime >= t - 0.25) {
        activeBtn = btn;
      }
    }
    chapterButtons.forEach((btn) => {
      btn.classList.toggle("is-active", btn === activeBtn);
    });
  }

  presetTabs.forEach((tab) => {
    tab.addEventListener("click", () => applyPreset(tab.getAttribute("data-preset")));
  });

  chapterButtons.forEach((btn) => {
    btn.addEventListener("click", () => {
      if (!video) return;
      const t = Number(btn.getAttribute("data-time") || "0");
      video.currentTime = t;
      video.play().catch(() => {});
      updateActiveChapter(t);
    });
  });

  if (video) {
    video.addEventListener("timeupdate", () => updateActiveChapter(video.currentTime));
  }

  const heroWatch = document.getElementById("hero-watch-btn");
  if (heroWatch && video) {
    heroWatch.addEventListener("click", () => {
      setTimeout(() => {
        video.play().catch(() => {});
      }, 280);
    });
  }

  if (copyBtn) {
    let timer = null;
    copyBtn.addEventListener("click", async () => {
      const text = PRESETS[currentPreset] || "";
      try {
        await navigator.clipboard.writeText(text);
        copyBtn.textContent = "Copied";
        copyBtn.classList.add("is-copied");
        clearTimeout(timer);
        timer = setTimeout(() => {
          copyBtn.classList.remove("is-copied");
          copyBtn.textContent = "Copy";
        }, 1800);
      } catch {
        const range = document.createRange();
        range.selectNodeContents(snippetEl);
        const sel = window.getSelection();
        sel.removeAllRanges();
        sel.addRange(range);
      }
    });
  }

  applyPreset(currentPreset);
})();
