(() => {
    "use strict";

    const clock = () => new Date().toLocaleTimeString("ja-JP", { hour12: false });
    const originalTitle = document.title;
    let titleTimer = null;
    let audio = null;
    let soundEnabled = false;
    const soundButton = document.getElementById("enable-sound");

    if (soundButton) {
        soundButton.addEventListener("click", async () => {
            // This tap gives the page user activation, which Chrome requires for vibrate().
            let vibrates = false;
            if (navigator.vibrate) {
                try { vibrates = navigator.vibrate(200); } catch (error) { vibrates = false; }
            }
            const AudioContext = window.AudioContext || window.webkitAudioContext;
            try {
                if (AudioContext) {
                    audio = audio || new AudioContext();
                    await audio.resume();
                    soundEnabled = true;
                }
            } catch (error) {
                soundEnabled = false;
            }
            if (!soundEnabled && !vibrates) {
                soundButton.textContent = "音・バイブを有効にできませんでした。もう一度お試しください";
                return;
            }
            soundButton.textContent = soundEnabled && vibrates
                ? "🔔 音とバイブを有効にしました"
                : soundEnabled
                    ? "🔔 音を有効にしました（この端末はバイブ非対応です）"
                    : "📳 バイブを有効にしました（この端末は音に非対応です）";
            soundButton.disabled = true;
        });
    }

    function notifyCalled() {
        if (navigator.vibrate) {
            try { navigator.vibrate([300, 100, 300]); } catch (error) { /* Device may disallow vibration. */ }
        }
        if (!titleTimer) {
            let highlighted = false;
            titleTimer = window.setInterval(() => {
                highlighted = !highlighted;
                document.title = highlighted ? "【呼出中】" : originalTitle;
            }, 1000);
            document.title = "【呼出中】";
        }
        if (soundEnabled && audio) {
            try {
                const oscillator = audio.createOscillator();
                const gain = audio.createGain();
                oscillator.connect(gain);
                gain.connect(audio.destination);
                oscillator.frequency.value = 880;
                gain.gain.setValueAtTime(0.15, audio.currentTime);
                gain.gain.exponentialRampToValueAtTime(0.001, audio.currentTime + 0.5);
                oscillator.start();
                oscillator.stop(audio.currentTime + 0.5);
            } catch (error) { /* The visual notification remains available. */ }
        }
    }

    function clearNotification() {
        window.clearInterval(titleTimer);
        titleTimer = null;
        document.title = originalTitle;
    }

    document.querySelectorAll("[data-poll-url]").forEach((element) => {
        const updated = document.querySelector(`[data-poll-updated-for="${element.id}"]`);
        const banner = document.querySelector(`[data-poll-error-for="${element.id}"]`);
        let lastUpdated = clock();
        let status = element.querySelector("[data-status]")?.dataset.status;
        let stopped = Number(element.dataset.pollInterval) === 0;
        let timer = null;
        let controller = null;
        let failures = 0;
        let generation = 0;

        function showUpdated() {
            if (updated) updated.textContent = `最終更新 ${lastUpdated}`;
        }
        showUpdated();

        function schedule(seconds) {
            window.clearTimeout(timer);
            if (!stopped && !document.hidden) timer = window.setTimeout(poll, seconds * 1000);
        }

        async function poll() {
            if (stopped || document.hidden) return;
            window.clearTimeout(timer);
            // Abort an earlier fetch and ignore its result, including its catch/finally.
            if (controller) controller.abort();
            controller = new AbortController();
            const currentController = controller;
            const currentGeneration = ++generation;
            const timeout = window.setTimeout(() => currentController.abort(), 10000);
            try {
                const response = await fetch(element.dataset.pollUrl, {
                    signal: currentController.signal,
                    cache: "no-store",
                    credentials: "same-origin",
                    headers: { "Accept": "text/html" }
                });
                if (!response.ok) throw new Error(`HTTP ${response.status}`);
                const html = await response.text();
                if (currentGeneration !== generation || document.hidden) return;
                element.innerHTML = html;
                lastUpdated = clock();
                showUpdated();
                if (banner) banner.hidden = true;
                failures = 0;
                const nextStatus = element.querySelector("[data-status]")?.dataset.status;
                if (status === "waiting" && nextStatus === "called") notifyCalled();
                if (nextStatus === "completed") clearNotification();
                status = nextStatus;
                const value = response.headers.get("X-Poll-Interval");
                const interval = value === null ? NaN : Number(value);
                stopped = interval === 0;
                schedule(Number.isFinite(interval) && interval > 0 ? interval : 15);
            } catch (error) {
                if (currentGeneration !== generation || document.hidden) return;
                if (banner) {
                    banner.textContent = `⚠ 接続できません（最終更新 ${lastUpdated}）。再接続を試みています…`;
                    banner.hidden = false;
                }
                schedule([15, 30, 60][Math.min(failures++, 2)]);
            } finally {
                window.clearTimeout(timeout);
                if (currentGeneration === generation) controller = null;
            }
        }

        function resume() {
            if (!stopped && !document.hidden) void poll();
        }
        document.addEventListener("visibilitychange", () => {
            window.clearTimeout(timer);
            if (document.hidden) {
                generation++;
                if (controller) controller.abort();
                controller = null;
            } else {
                resume();
            }
        });
        window.addEventListener("online", resume);
        // HTML already contains the initial server-rendered result.
        schedule(Number(element.dataset.pollInterval) || 15);
    });
})();
