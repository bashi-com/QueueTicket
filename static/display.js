(() => {
    "use strict";

    function detectNewCalledItems(prevState, nextState) {
        if (!prevState || !nextState) return [];
        const knownSeqs = new Set();
        for (const item of prevState.calling || []) {
            knownSeqs.add(item.seq);
        }
        for (const item of prevState.absent || []) {
            knownSeqs.add(item.seq);
        }
        const newlyCalled = [];
        for (const item of nextState.calling || []) {
            if (!knownSeqs.has(item.seq)) {
                newlyCalled.push(item);
            }
        }
        return newlyCalled;
    }

    function mergeOverlayNumbers(existingNumbers, newItems) {
        const merged = existingNumbers ? Array.from(existingNumbers) : [];
        for (const item of newItems || []) {
            const num = typeof item === "number" ? item : item.number;
            if (typeof num === "number" && !merged.includes(num)) {
                merged.push(num);
            }
        }
        return merged;
    }

    function formatOverlayHeadline(numbers) {
        return (numbers || []).map((n) => `No.${n}`).join(" / ") + " の方";
    }

    if (typeof window !== "undefined") {
        window.QueueDisplay = {
            detectNewCalledItems,
            mergeOverlayNumbers,
            formatOverlayHeadline
        };
    }

    if (typeof document === "undefined" || !document.body || !document.body.dataset) {
        return;
    }

    const mode = document.body.dataset.display;
    if (mode !== "front" && mode !== "screen") {
        return;
    }

    const pollIntervalMs = mode === "front" ? 2000 : 3000;
    const connStatus = document.getElementById("display-conn-status");

    // Screen Wake Lock
    let wakeLock = null;
    async function requestWakeLock() {
        if (typeof navigator === "undefined" || !navigator.wakeLock || typeof navigator.wakeLock.request !== "function") {
            return;
        }
        try {
            wakeLock = await navigator.wakeLock.request("screen");
        } catch (error) {
            /* Ignore wakeLock errors */
        }
    }
    void requestWakeLock();

    // Front display elements
    const frontTicketView = document.getElementById("front-ticket-view");
    const frontIdleView = document.getElementById("front-idle-view");
    const frontNumber = document.getElementById("front-number");
    const frontGroup = document.getElementById("front-group");
    const frontQr = document.getElementById("front-qr");
    const frontViewed = document.getElementById("front-viewed");
    const frontWaiting = document.getElementById("front-waiting");
    const frontCalling = document.getElementById("front-calling");
    const frontAbsent = document.getElementById("front-absent");
    let lastQrKey = null;

    function renderFront(state) {
        if (state.front) {
            if (frontIdleView) frontIdleView.hidden = true;
            if (frontTicketView) frontTicketView.hidden = false;
            if (frontNumber) frontNumber.textContent = String(state.front.number);
            if (frontGroup) frontGroup.textContent = `${state.front.group_size}名様`;
            const qrKey = `${state.front.seq}:${state.front.qr_svg}`;
            if (frontQr && qrKey !== lastQrKey) {
                frontQr.innerHTML = state.front.qr_svg;
                lastQrKey = qrKey;
            }
            if (frontViewed) {
                frontViewed.hidden = state.front.phase !== "viewed";
            }
        } else {
            if (frontTicketView) frontTicketView.hidden = true;
            if (frontIdleView) frontIdleView.hidden = false;
            if (frontViewed) frontViewed.hidden = true;
            lastQrKey = null;
            if (frontWaiting) {
                frontWaiting.textContent = `現在の待ち ${state.waiting_count}組`;
            }
            if (frontCalling) {
                const callingNums = (state.calling || []).map((item) => item.number);
                if (callingNums.length > 0) {
                    frontCalling.textContent = `お呼び出し中: ${callingNums.join(", ")}`;
                    frontCalling.hidden = false;
                } else {
                    frontCalling.textContent = "";
                    frontCalling.hidden = true;
                }
            }
            if (frontAbsent) {
                const absentNums = (state.absent || []).map((item) => item.number);
                if (absentNums.length > 0) {
                    frontAbsent.textContent = `お呼びした番号: ${absentNums.join(", ")} — 戻られた方はスマートフォンのボタンかスタッフへ`;
                    frontAbsent.hidden = false;
                } else {
                    frontAbsent.textContent = "";
                    frontAbsent.hidden = true;
                }
            }
        }
    }

    // Screen display elements & state
    const screenContainer = document.getElementById("screen-container");
    const videoPane = document.getElementById("video-pane");
    const pvVideo = document.getElementById("pv-video");
    const screenClock = document.getElementById("screen-clock");
    const screenWaiting = document.getElementById("screen-waiting");
    const screenCallingSection = document.getElementById("screen-calling-section");
    const screenUpcomingSection = document.getElementById("screen-upcoming-section");
    const screenAbsentSection = document.getElementById("screen-absent-section");
    const screenCalling = document.getElementById("screen-calling");
    const screenUpcoming = document.getElementById("screen-upcoming");
    const screenAbsent = document.getElementById("screen-absent");
    const callOverlay = document.getElementById("call-overlay");
    const overlayNumbersEl = document.getElementById("overlay-numbers");
    const setupModal = document.getElementById("setup-modal");
    const pvFileInput = document.getElementById("pv-file");
    const pvSoundCheckbox = document.getElementById("pv-sound");
    const startWithVideoBtn = document.getElementById("start-with-video");
    const startNoVideoBtn = document.getElementById("start-no-video");
    const openSetupBtn = document.getElementById("open-setup");

    let audioCtx = null;
    let currentVideoObjectUrl = null;
    let prevScreenState = null;
    let activeOverlayNumbers = [];
    let overlayTimer = null;

    function ensureAudioContext() {
        const AudioContextClass = window.AudioContext || window.webkitAudioContext;
        if (!AudioContextClass) return null;
        try {
            audioCtx = audioCtx || new AudioContextClass();
            void audioCtx.resume();
        } catch (error) {
            /* Ignore audio errors */
        }
        return audioCtx;
    }

    function playChime() {
        if (!audioCtx) return;
        try {
            const now = audioCtx.currentTime;
            const tones = [
                { freq: 880, start: now, duration: 0.4 },
                { freq: 660, start: now + 0.4, duration: 0.4 }
            ];
            for (const tone of tones) {
                const osc = audioCtx.createOscillator();
                const gain = audioCtx.createGain();
                osc.connect(gain);
                gain.connect(audioCtx.destination);
                osc.frequency.value = tone.freq;
                gain.gain.setValueAtTime(0.2, tone.start);
                gain.gain.exponentialRampToValueAtTime(0.001, tone.start + tone.duration);
                osc.start(tone.start);
                osc.stop(tone.start + tone.duration);
            }
        } catch (error) {
            /* Ignore chime failure */
        }
    }

    function requestFullscreenSafe() {
        const root = document.documentElement;
        if (root && typeof root.requestFullscreen === "function") {
            try {
                const promise = root.requestFullscreen();
                if (promise && typeof promise.catch === "function") {
                    promise.catch(() => {});
                }
            } catch (error) {
                /* Ignore fullscreen restriction */
            }
        }
    }

    function applyVideoSoundSetting() {
        if (!pvVideo) return;
        const soundOn = pvSoundCheckbox ? Boolean(pvSoundCheckbox.checked) : true;
        pvVideo.muted = !soundOn;
        if (callOverlay && !callOverlay.hidden) {
            pvVideo.volume = 0.2;
        } else {
            pvVideo.volume = 1.0;
        }
    }

    function startScreen(withVideo) {
        ensureAudioContext();
        requestFullscreenSafe();
        const file = pvFileInput && pvFileInput.files && pvFileInput.files[0];
        if (withVideo && file && pvVideo && typeof URL !== "undefined" && typeof URL.createObjectURL === "function") {
            if (currentVideoObjectUrl && typeof URL.revokeObjectURL === "function") {
                URL.revokeObjectURL(currentVideoObjectUrl);
            }
            currentVideoObjectUrl = URL.createObjectURL(file);
            pvVideo.src = currentVideoObjectUrl;
            applyVideoSoundSetting();
            if (videoPane) videoPane.hidden = false;
            if (screenContainer && screenContainer.classList) {
                screenContainer.classList.remove("no-video");
            }
            try {
                const playPromise = pvVideo.play();
                if (playPromise && typeof playPromise.catch === "function") {
                    playPromise.catch(() => {});
                }
            } catch (error) {
                /* Ignore video play errors */
            }
        } else {
            if (pvVideo) {
                try { if (typeof pvVideo.pause === "function") pvVideo.pause(); } catch (error) { /* ignore */ }
                pvVideo.removeAttribute?.("src");
            }
            if (videoPane) videoPane.hidden = true;
            if (screenContainer && screenContainer.classList) {
                screenContainer.classList.add("no-video");
            }
        }
        if (setupModal) setupModal.hidden = true;
    }

    if (mode === "screen") {
        if (startWithVideoBtn && startWithVideoBtn.addEventListener) {
            startWithVideoBtn.addEventListener("click", () => startScreen(true));
        }
        if (startNoVideoBtn && startNoVideoBtn.addEventListener) {
            startNoVideoBtn.addEventListener("click", () => startScreen(false));
        }
        if (pvSoundCheckbox && pvSoundCheckbox.addEventListener) {
            pvSoundCheckbox.addEventListener("change", applyVideoSoundSetting);
        }
        if (openSetupBtn && openSetupBtn.addEventListener) {
            openSetupBtn.addEventListener("click", () => {
                if (setupModal) setupModal.hidden = false;
            });
        }

        // Hide mouse cursor after 3 seconds of inactivity
        let cursorTimer = null;
        const resetCursorTimer = () => {
            if (document.body.classList) {
                document.body.classList.remove("hide-cursor");
            }
            window.clearTimeout(cursorTimer);
            cursorTimer = window.setTimeout(() => {
                if (document.body.classList) {
                    document.body.classList.add("hide-cursor");
                }
            }, 3000);
        };
        if (document.addEventListener) {
            document.addEventListener("mousemove", resetCursorTimer);
            document.addEventListener("mousedown", resetCursorTimer);
            document.addEventListener("keydown", resetCursorTimer);
        }
        resetCursorTimer();
    }

    function renderNumberBadges(section, container, items, badgeClass, showRemaining) {
        const list = items || [];
        if (section) {
            section.hidden = list.length === 0;
        }
        if (!container) return;
        if (typeof container.replaceChildren === "function") {
            container.replaceChildren();
        } else {
            container.innerHTML = "";
        }
        if (container.classList) {
            if (list.length > 4) {
                container.classList.add("many");
            } else {
                container.classList.remove("many");
            }
        }
        if (list.length === 0) {
            return;
        }
        if (typeof document.createElement === "function" && typeof container.appendChild === "function") {
            for (const item of list) {
                const badge = document.createElement("span");
                badge.className = badgeClass;
                badge.textContent = String(item.number);
                if (showRemaining && typeof item.remaining === "number" && item.remaining < item.group_size) {
                    if (badge.style) {
                        badge.style.display = "inline-flex";
                        badge.style.flexDirection = "column";
                        badge.style.alignItems = "center";
                        badge.style.gap = "0.05em";
                        badge.style.lineHeight = "1.05";
                    }
                    const small = document.createElement("small");
                    small.textContent = `残り${item.remaining}名`;
                    if (small.style) small.style.fontSize = "0.32em";
                    badge.appendChild(small);
                }
                container.appendChild(badge);
            }
        } else {
            container.textContent = list.map((item) => item.number).join(", ");
        }
    }

    function triggerOverlay(newItems) {
        if (!newItems || newItems.length === 0) return;
        activeOverlayNumbers = mergeOverlayNumbers(activeOverlayNumbers, newItems);
        if (overlayNumbersEl) {
            overlayNumbersEl.textContent = formatOverlayHeadline(activeOverlayNumbers);
        }
        if (callOverlay) {
            callOverlay.hidden = false;
        }
        if (pvVideo) {
            pvVideo.volume = 0.2;
        }
        playChime();
        window.clearTimeout(overlayTimer);
        overlayTimer = window.setTimeout(() => {
            activeOverlayNumbers = [];
            if (callOverlay) callOverlay.hidden = true;
            if (pvVideo) pvVideo.volume = 1.0;
        }, 8000);
    }

    function renderScreen(state) {
        if (screenClock) screenClock.textContent = state.clock || "--:--";
        if (screenWaiting) screenWaiting.textContent = `待ち ${state.waiting_count}組`;
        renderNumberBadges(screenCallingSection, screenCalling, state.calling, "calling-number", true);
        renderNumberBadges(screenUpcomingSection, screenUpcoming, state.upcoming, "upcoming-number", false);
        renderNumberBadges(screenAbsentSection, screenAbsent, state.absent, "absent-number", false);

        if (prevScreenState !== null) {
            const newlyCalled = detectNewCalledItems(prevScreenState, state);
            if (newlyCalled.length > 0) {
                triggerOverlay(newlyCalled);
            }
        }
        prevScreenState = state;
    }

    // Polling loop
    let pollTimer = null;
    let controller = null;
    let generation = 0;

    function scheduleNext() {
        window.clearTimeout(pollTimer);
        if (!document.hidden) {
            pollTimer = window.setTimeout(poll, pollIntervalMs);
        }
    }

    async function poll() {
        if (document.hidden) return;
        window.clearTimeout(pollTimer);
        if (controller) controller.abort();
        controller = new AbortController();
        const currentController = controller;
        const currentGeneration = ++generation;
        const timeout = window.setTimeout(() => currentController.abort(), 10000);
        try {
            const response = await fetch("/admin/display/state", {
                signal: currentController.signal,
                cache: "no-store",
                credentials: "same-origin",
                headers: { "Accept": "application/json" }
            });
            if (!response.ok) throw new Error(`HTTP ${response.status}`);
            const state = await response.json();
            if (currentGeneration !== generation || document.hidden) return;
            if (connStatus) connStatus.hidden = true;
            if (mode === "front") {
                renderFront(state);
            } else {
                renderScreen(state);
            }
            scheduleNext();
        } catch (error) {
            if (currentGeneration !== generation || document.hidden) return;
            if (connStatus) {
                connStatus.textContent = "⚠ 接続中…";
                connStatus.hidden = false;
            }
            scheduleNext();
        } finally {
            window.clearTimeout(timeout);
            if (currentGeneration === generation) controller = null;
        }
    }

    if (document.addEventListener) {
        document.addEventListener("visibilitychange", () => {
            window.clearTimeout(pollTimer);
            if (document.hidden) {
                generation++;
                if (controller) controller.abort();
                controller = null;
            } else {
                void requestWakeLock();
                void poll();
            }
        });
    }
    if (window.addEventListener) {
        window.addEventListener("online", () => {
            if (!document.hidden) void poll();
        });
    }

    void poll();
})();
