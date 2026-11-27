(() => {
  const I18N = {
    ua: {
      pageTitle: "Anaconda — 4 ГБ VRAM & No-RT форк VIctory / cool_fox",
      navDemo: "Відео",
      navTelemetry: "Заміри",
      navArch: "Архітектура",
      navStart: "Запуск",
      navRepo: "GitHub",
      heroSub: "4 ГБ VRAM. Без апаратного Ray Tracing.",
      heroLead:
        "Інженерний форк VIctory / cool_fox на Rust і Direct3D 12, оптимізований для бюджетних відеокарт рівня GTX 1650 4 ГБ: жорсткий ліміт VRAM, 16 МіБ best-fit тайлові хіпи, вбудований Series S FSR + RCAS та генерація кадрів у стилі AMD FSR 3.",
      heroWatch: "Дивитись запис GTX 1650",
      heroRepo: "Відкрити репозиторій",
      heroNotice:
        "Репозиторій не містить файлів, ресурсів чи бінарників гри. Потрібен власний дамп Xbox Series під час запуску.",
      rigGpu: "Відеокарта тесту",
      rigCpu: "Процесор і RAM",
      rigRt: "Ray Tracing",
      rigRtVal: "Не потрібен (No-RT)",
      rigProfile: "Консольний профіль",
      theaterHeading: "Реальний запис геймплею — GTX 1650 4 ГБ (чисті FPS, без FrameGen)",
      theaterSpecs: "1080p запис · 57.5 с · 4–5 FPS (211–260 ms) · HUD F4",
      ch0: "Двір і човен",
      ch1: "Пікап і світло",
      ch2: "Вулиця на заході",
      ch3: "Колесо зброї (UI)",
      ch4: "Авто та тіні",
      downloadMp4: "Завантажити MP4 (29.3 МБ)",
      releaseLink: "Реліз v0.1-demo",
      telTitle: "Реальні заміри на GTX 1650 4 ГБ",
      telDesc:
        "Усі цифри взяті напряму з релізного профайлера victory_gpu::performance без прикрашань і без генерації кадрів.",
      m1Name: "Час кадру та реальні FPS",
      m1Note: "Відкритий світ, Frame Generation вимкнено",
      m1Now: "196–250 ms (4.0–5.1 FPS)",
      m1Delta: "було 300–345 ms (3.1 FPS)",
      m2Name: "Тайлові хіпи текстур у VRAM",
      m2Note: "16 МіБ best-fit пул замість 64 МіБ first-fit",
      m2Now: "480 MiB",
      m2Delta: "-352 MiB проти 832 MiB",
      m3Name: "Простій GPU (gpu_wait_ms)",
      m3Note: "Stage-2 relay + перша фаза відкритого світу",
      m3Now: "56–70 ms",
      m3Delta: "в 3× нижче за 187–226 ms",
      m4Name: "Підготовка текстур (texture_prepare)",
      m4Note: "Ліміт міпів 256 px (staging 88 KiB замість 5.4 MiB)",
      m4Now: "0.6–1.9 ms",
      m4Delta: "~80× швидше за 138 ms",
      m5Name: "Прохід Frame Generation (720p)",
      m5Note: "4-рівневий Optical Flow + синтез на GTX 1650",
      m5Now: "1.20 ms / кадр",
      m5Delta: "~8.2 MiB VRAM, без HUD-гостингу",
      lblBefore: "ДО",
      lblAfter: "ЗАРАЗ",
      lblGpu: "GTX 1650",
      roadmapTitle: "Наступна ціль профілювання — 10 реальних FPS без FrameGen",
      roadmapText:
        "Релізний профайлер уже показав 4 конкретні джерела решти ~240 ms/кадр: гра досі тримає 1080p render targets (rt_mib = 1,466 MiB), що перевищує бюджет 3.0 ГБ, а також фази CPU/readback: deferred_ms (35–55 ms), card_back_ms (32–45 ms) і snap_ms (22–27 ms). Саме над цим працюємо в наступному етапі.",
      archTitle: "Що змінює Anaconda",
      archLead:
        "Код побудовано поверх codec-d/cool_fox (pareto-residency) і VIctory з прицілом на Turing 4 ГБ без RT-ядер.",
      a1Title: "Жорсткий контроль 4 ГБ VRAM та резерви під світ",
      a1Body:
        "VICTORY_VRAM_MB обмежує локальний бюджет DXGI, а resource::room() більше не ігнорує реальне заповнення карти при заданому ліміті текстур. Для render targets і GPU-буферів завчасно тримається вільний резерв, а дрібні буфери від 64 KiB залишаються у VRAM замість повільного PCIe Shared RAM.",
      a2Title: "16 МіБ best-fit тайлові хіпи текстур",
      a2Body:
        "Замість 64-мегабайтних хіпів, у яких «зависало» до 376 MiB мертвої VRAM, на картах ≤ 6 ГБ автоматично створюються 16 МіБ хіпи з best-fit аллокацією, що скоротило обсяг пулу з 832 MiB до 480 MiB із щільністю пакування 96%.",
      a3Title: "Вбудований Series S FSR + RCAS без зовнішніх DLL",
      a3Body:
        "Повна реалізація amd_fidelityfx_loader_xbox.dll на Rust + D3D12 compute-шейдер cs_5_1 із 5-tap clamped RCAS різкістю, захистом від NaN/Inf і підтримкою багатопотокових thin command lists.",
      a4Title: "Генерація кадрів у стилі AMD FSR 3 (HUD-less)",
      a4Body:
        "4-рівнева піраміда двонаправленого Zero-Mean SAD оптичного потоку, детектор зміни сцени на GPU та 3-гіпотезний синтез із обробкою дизоклюзій. Працює строго по 3D-площині (Plane 0) до накладання інтерфейсу (Plane 1), тому HUD і субтитри не розмиваються.",
      a5Title: "Програмна компенсація освітлення без Ray Tracing",
      a5Body:
        "Режим без апаратних RT-арен (VICTORY_RT_NO_ARENAS=1) із програмною заміною непрямого дифузного освітлення та відбиттів (CS_RayMiss), налаштований так, щоб персонажі в приміщеннях не перетворювались на чорні силуети.",
      a6Title: "Вбудований FPS / MS / VRAM HUD у кутку екрана",
      a6Body:
        "Нульові витрати VRAM у релізі, миттєве перемикання клавішею F4 у грі. Відображає реальні FPS, час кадру, статус FrameGen та поточне споживання відеопам’яті прямо поверх фінального кадру.",
      startTitle: "Збірка та запуск",
      startDesc:
        "Обери готовий режим запуску нижче, підстав шлях до власного дампу Xbox Series і запускай Play-Anaconda.cmd.",
      tabNative: "Чисті FPS (профілювання)",
      tabFg: "2× FrameGen (плавність)",
      tabUltraLow: "Максимум запасу VRAM",
      copy: "Скопіювати",
      copied: "Скопійовано",
      reqTitle: "Мінімальні вимоги",
      r1: "<strong>ОС:</strong> Windows 10 / 11 x64",
      r2: "<strong>CPU:</strong> процесор із підтримкою AVX2 (перевірено на Ryzen 5 5500)",
      r3: "<strong>GPU:</strong> Direct3D 12, Shader Model 6.6 + Mesh Shaders, від 4 ГБ VRAM (апаратний RT не потрібен)",
      r4: "<strong>Інструменти збірки:</strong> Rust (з rust-toolchain.toml), MSVC Build Tools, Windows SDK",
      r5: "<strong>Дані гри:</strong> власний дамп Xbox Series (у репозиторії немає жодних файлів гри)",
      thVar: "Змінна",
      thDef: "За замовчуванням",
      thDesc: "Призначення",
      e1: "Режим екрана, який DXGI передає грі",
      e2: "Обмеження локального бюджету DXGI для 4 ГБ карт (MiB)",
      e3: "Розмір хіпів тайлових текстур (16 MiB best-fit для ≤ 6 ГБ GPU)",
      e4: "Верхній ліміт текстур у VRAM із дотриманням DXGI-бюджету",
      e5: "Stage-2 асинхронний relay із перевіркою точних діапазонів",
      e6: "Масштаб рендеру FSR + сила різкості RCAS",
      e7: "Генерація кадрів у стилі AMD FSR 3 (0=вимк, 1=2×, 3=3×, 4=4×)",
      e8: "Оверлей FPS / MS / VRAM на екрані (перемикання клавішею F4)",
      footerCredits:
        "Anaconda базується на роботі спільноти VIctory та codec-d/cool_fox (pareto-residency), а також внесках Marvisak, fxceless, DexNom, Belobug, myokex, FQY55kai, Geckomacabre та cxy0072019-sketch. Ліцензія GPL-3.0-or-later.",
      footerRepo: "Репозиторій",
      footerReleases: "Релізи",
      footerPolicy: "Політика асетів"
    },
    en: {
      pageTitle: "Anaconda — 4 GB VRAM & No-RT fork of VIctory / cool_fox",
      navDemo: "Capture",
      navTelemetry: "Benchmarks",
      navArch: "Architecture",
      navStart: "Quick Start",
      navRepo: "GitHub",
      heroSub: "4 GB VRAM. No hardware Ray Tracing.",
      heroLead:
        "An engineering fork of VIctory / cool_fox in Rust and Direct3D 12 tuned for 4 GB GPUs like the GTX 1650: strict VRAM budgeting, 16 MiB best-fit tile heaps, built-in Series S FSR + RCAS, and HUD-less AMD FSR 3-style Frame Generation.",
      heroWatch: "Watch GTX 1650 capture",
      heroRepo: "Open repository",
      heroNotice:
        "This repository contains no game files, binaries, or assets. You must supply your own Xbox Series dump at runtime.",
      rigGpu: "Test GPU",
      rigCpu: "CPU & Memory",
      rigRt: "Ray Tracing",
      rigRtVal: "Not required (No-RT)",
      rigProfile: "Console profile",
      theaterHeading: "Real gameplay capture — GTX 1650 4 GB (Native FPS, FrameGen OFF)",
      theaterSpecs: "1080p capture · 57.5s · 4–5 FPS (211–260 ms) · F4 HUD",
      ch0: "Yard & boat",
      ch1: "Pickup & lighting",
      ch2: "Sunset street run",
      ch3: "Weapon wheel (UI)",
      ch4: "Vehicles & shadows",
      downloadMp4: "Download MP4 (29.3 MB)",
      releaseLink: "Release v0.1-demo",
      telTitle: "Measured telemetry on GTX 1650 4 GB",
      telDesc:
        "Every number comes straight from the release victory_gpu::performance profiler with Frame Generation disabled.",
      m1Name: "Frame time & native FPS",
      m1Note: "Open world street gameplay, Frame Generation OFF",
      m1Now: "196–250 ms (4.0–5.1 FPS)",
      m1Delta: "down from 300–345 ms (3.1 FPS)",
      m2Name: "Tiled texture VRAM heaps",
      m2Note: "16 MiB best-fit pool replacing 64 MiB first-fit",
      m2Now: "480 MiB",
      m2Delta: "-352 MiB vs 832 MiB",
      m3Name: "GPU pipeline wait (gpu_wait_ms)",
      m3Note: "Stage-2 exact relay in early open world",
      m3Now: "56–70 ms",
      m3Delta: "3× lower than 187–226 ms",
      m4Name: "Texture upload prep (texture_prepare)",
      m4Note: "256 px mip cap (88 KiB staging vs 5.4 MiB)",
      m4Now: "0.6–1.9 ms",
      m4Delta: "~80× faster than 138 ms",
      m5Name: "Frame Generation GPU pass (720p)",
      m5Note: "4-level Optical Flow + synthesis on GTX 1650",
      m5Now: "1.20 ms / frame",
      m5Delta: "~8.2 MiB VRAM, zero HUD ghosting",
      lblBefore: "BEFORE",
      lblAfter: "NOW",
      lblGpu: "GTX 1650",
      roadmapTitle: "Next profiling milestone — 10 native FPS without FrameGen",
      roadmapText:
        "Our release profiler isolated the exact remaining ~240 ms/frame: the title still allocates 1080p render targets (rt_mib = 1,466 MiB) pushing local VRAM above the 3.0 GB budget, alongside three CPU/readback phases: deferred_ms (35–55 ms), card_back_ms (32–45 ms), and snap_ms (22–27 ms). Those are the next targets.",
      archTitle: "What Anaconda changes",
      archLead:
        "Built on codec-d/cool_fox (pareto-residency) and VIctory, focused on keeping 4 GB Turing GPUs playable without RT cores.",
      a1Title: "Strict 4 GB VRAM discipline & world reserves",
      a1Body:
        "VICTORY_VRAM_MB caps the host DXGI budget, and resource::room() enforces real VRAM usage plus render-target and buffer reserves even when VICTORY_TEXTURE_BUDGET_MB is set. Shader-written buffers down to 64 KiB stay resident in VRAM instead of crossing PCIe Shared RAM.",
      a2Title: "16 MiB best-fit tiled texture pools",
      a2Body:
        "Instead of 64 MiB pools stranding up to 376 MiB of dead VRAM, <= 6 GiB GPUs automatically allocate 16 MiB heaps with best-fit packing—cutting live heap footprint from 832 MiB down to 480 MiB at 96% utilization.",
      a3Title: "Zero-dependency Series S FSR + RCAS upscaler",
      a3Body:
        "Implements amd_fidelityfx_loader_xbox.dll natively in Rust backed by a D3D12 cs_5_1 compute shader with 5-tap clamped RCAS sharpening, NaN/Inf recovery, and multi-threaded thin command-list translation.",
      a4Title: "AMD FSR 3-style Optical Flow Frame Generation",
      a4Body:
        "A 4-level coarse-to-fine Zero-Mean SAD bidirectional optical flow pyramid, GPU scene-cut detector, and 3-hypothesis disocclusion-aware synthesizer operating strictly on the HUD-less 3D plane (Plane 0) before UI compositing (Plane 1).",
      a5Title: "Software indirect lighting without Ray Tracing",
      a5Body:
        "Zero-VRAM No-RT mode (VICTORY_RT_NO_ARENAS=1) paired with software diffuse GI and CS_RayMiss specular reflections stand-in so indoor characters and interiors remain legible without RT hardware.",
      a6Title: "Zero-VRAM on-screen FPS / MS / VRAM HUD",
      a6Body:
        "Toggle live at any time with F4. Renders native FPS, generated FPS (when FrameGen is active), frametime, and live VRAM usage directly onto the final swap-chain frame.",
      startTitle: "Build & run",
      startDesc:
        "Choose a launch profile below, point VICTORY_GAME_DIR to your own Xbox Series dump, and run Play-Anaconda.cmd.",
      tabNative: "Native FPS (baseline)",
      tabFg: "2× FrameGen (smooth)",
      tabUltraLow: "Max VRAM headroom",
      copy: "Copy",
      copied: "Copied",
      reqTitle: "Requirements",
      r1: "<strong>OS:</strong> Windows 10 / 11 x64",
      r2: "<strong>CPU:</strong> AVX2-capable processor (tested on Ryzen 5 5500)",
      r3: "<strong>GPU:</strong> Direct3D 12 with Shader Model 6.6 & Mesh Shaders, 4 GB+ VRAM (no RT cores required)",
      r4: "<strong>Toolchain:</strong> Rust (pinned in rust-toolchain.toml), MSVC C++ Build Tools, Windows SDK",
      r5: "<strong>Game data:</strong> Your own Xbox Series dump (zero game files are stored in this repo)",
      thVar: "Variable",
      thDef: "Default",
      thDesc: "Purpose",
      e1: "Display resolution reported to the title via DXGI",
      e2: "Caps local DXGI VRAM budget for 4 GB cards (MiB)",
      e3: "Tiled texture heap granularity (16 MiB best-fit on <= 6 GiB GPUs)",
      e4: "Upper cap on texture VRAM while honoring DXGI & RT/buffer reserves",
      e5: "Stage-2 per-range async relay avoiding full-flight GPU stalls",
      e6: "Internal FSR scale factor and RCAS sharpening strength",
      e7: "AMD FSR 3-style Optical Flow Frame Generation (0=off, 1=2x, 3=3x, 4=4x)",
      e8: "On-screen FPS / MS / VRAM overlay (live toggle with F4)",
      footerCredits:
        "Anaconda builds on the VIctory community and codec-d/cool_fox (pareto-residency), including work by Marvisak, fxceless, DexNom, Belobug, myokex, FQY55kai, Geckomacabre, and cxy0072019-sketch. Licensed under GPL-3.0-or-later.",
      footerRepo: "Repository",
      footerReleases: "Releases",
      footerPolicy: "Asset policy"
    }
  };

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

  let currentLang = localStorage.getItem("anaconda-lang") === "en" ? "en" : "ua";
  let currentPreset = "native";

  const video = document.getElementById("demo-video");
  const chapterButtons = Array.from(document.querySelectorAll(".chapter-btn"));
  const langButtons = Array.from(document.querySelectorAll(".lang-btn"));
  const presetTabs = Array.from(document.querySelectorAll(".preset-tab"));
  const snippetEl = document.getElementById("preset-code");
  const copyBtn = document.getElementById("copy-code-btn");

  function applyLanguage(lang) {
    currentLang = lang;
    localStorage.setItem("anaconda-lang", lang);
    document.documentElement.lang = lang === "ua" ? "uk" : "en";
    const dict = I18N[lang];
    document.title = dict.pageTitle;

    document.querySelectorAll("[data-i18n]").forEach((el) => {
      const key = el.getAttribute("data-i18n");
      if (dict[key] !== undefined) {
        el.textContent = dict[key];
      }
    });

    document.querySelectorAll("[data-i18n-html]").forEach((el) => {
      const key = el.getAttribute("data-i18n-html");
      if (dict[key] !== undefined) {
        el.innerHTML = dict[key];
      }
    });

    langButtons.forEach((btn) => {
      const active = btn.getAttribute("data-lang") === lang;
      btn.classList.toggle("is-active", active);
      btn.setAttribute("aria-pressed", String(active));
    });

    if (copyBtn && !copyBtn.classList.contains("is-copied")) {
      copyBtn.textContent = dict.copy;
    }
  }

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

  langButtons.forEach((btn) => {
    btn.addEventListener("click", () => applyLanguage(btn.getAttribute("data-lang")));
  });

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
        copyBtn.textContent = I18N[currentLang].copied;
        copyBtn.classList.add("is-copied");
        clearTimeout(timer);
        timer = setTimeout(() => {
          copyBtn.classList.remove("is-copied");
          copyBtn.textContent = I18N[currentLang].copy;
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

  applyLanguage(currentLang);
  applyPreset(currentPreset);
})();
