// Run with: node tests/display.cjs (no npm packages required).
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");
const source = fs.readFileSync(`${__dirname}/../static/display.js`, "utf8");

function makeElement(id, initial = {}) {
    const classes = new Set(initial.classes || []);
    const children = [];
    const listeners = {};
    return {
        id,
        hidden: Boolean(initial.hidden),
        textContent: initial.textContent || "",
        innerHTML: initial.innerHTML || "",
        className: "",
        volume: 1.0,
        muted: false,
        loop: Boolean(initial.loop),
        src: "",
        playCount: 0,
        checked: initial.checked !== undefined ? initial.checked : true,
        files: initial.files || [],
        children,
        listeners,
        classList: {
            add: (c) => classes.add(c),
            remove: (c) => classes.delete(c),
            contains: (c) => classes.has(c)
        },
        replaceChildren() {
            children.length = 0;
            this.textContent = "";
        },
        appendChild(child) {
            children.push(child);
            this.textContent = children.map((c) => c.textContent).join(", ");
        },
        addEventListener(name, cb) {
            listeners[name] = cb;
        },
        play() {
            this.playCount += 1;
            return Promise.resolve();
        },
        pause() {},
        removeAttribute() {}
    };
}

function setup(mode = "screen") {
    const timers = new Map();
    const docEvents = {};
    const winEvents = {};
    const replies = [];
    const requests = [];
    const revoked = [];
    let timerId = 0;
    let chimes = 0;

    const elements = {
        "display-conn-status": makeElement("display-conn-status", { hidden: true }),
        // front elements
        "front-ticket-view": makeElement("front-ticket-view", { hidden: true }),
        "front-idle-view": makeElement("front-idle-view", { hidden: false }),
        "front-number": makeElement("front-number"),
        "front-group": makeElement("front-group"),
        "front-qr": makeElement("front-qr"),
        "front-viewed": makeElement("front-viewed", { hidden: true }),
        "front-waiting": makeElement("front-waiting", { textContent: "現在の待ち 0組" }),
        "front-calling": makeElement("front-calling", { hidden: true }),
        "front-absent": makeElement("front-absent", { hidden: true }),
        // screen elements
        "screen-container": makeElement("screen-container", { classes: ["no-video"] }),
        "video-pane": makeElement("video-pane", { hidden: true }),
        "pv-video": makeElement("pv-video"),
        "screen-clock": makeElement("screen-clock", { textContent: "--:--" }),
        "screen-waiting": makeElement("screen-waiting", { textContent: "待ち 0組" }),
        "screen-calling-section": makeElement("screen-calling-section", { hidden: true }),
        "screen-upcoming-section": makeElement("screen-upcoming-section", { hidden: true }),
        "screen-absent-section": makeElement("screen-absent-section", { hidden: true }),
        "screen-calling": makeElement("screen-calling"),
        "screen-upcoming": makeElement("screen-upcoming"),
        "screen-absent": makeElement("screen-absent"),
        "call-overlay": makeElement("call-overlay", { hidden: true }),
        "overlay-numbers": makeElement("overlay-numbers"),
        "setup-modal": makeElement("setup-modal", { hidden: false }),
        "pv-file": makeElement("pv-file"),
        "pv-sound": makeElement("pv-sound", { checked: true }),
        "start-with-video": makeElement("start-with-video"),
        "start-no-video": makeElement("start-no-video"),
        "open-setup": makeElement("open-setup")
    };

    const document = {
        hidden: false,
        documentElement: {
            requestFullscreen: () => Promise.resolve()
        },
        body: {
            dataset: { display: mode },
            classList: {
                add() {},
                remove() {}
            }
        },
        getElementById: (id) => elements[id] || null,
        createElement: (tag) => makeElement(tag),
        addEventListener: (name, cb) => { docEvents[name] = cb; }
    };

    class AudioContext {
        currentTime = 0;
        destination = {};
        resume() { return Promise.resolve(); }
        createOscillator() {
            return {
                frequency: { value: 0 },
                connect() {},
                start() { chimes++; },
                stop() {}
            };
        }
        createGain() {
            return {
                connect() {},
                gain: { setValueAtTime() {}, exponentialRampToValueAtTime() {} }
            };
        }
    }

    const window = {
        AudioContext,
        setTimeout(callback, milliseconds) {
            const id = ++timerId;
            timers.set(id, { callback, milliseconds });
            return id;
        },
        clearTimeout: (id) => timers.delete(id),
        addEventListener: (name, cb) => { winEvents[name] = cb; }
    };

    const context = {
        document,
        window,
        AbortController,
        URL: {
            createObjectURL: (file) => `blob:${file && file.name ? file.name : "test-video"}`,
            revokeObjectURL: (url) => { revoked.push(url); }
        },
        navigator: {
            wakeLock: {
                request: () => Promise.resolve({ release: () => Promise.resolve() })
            }
        },
        fetch(url, options) {
            requests.push({ url, options });
            assert.ok(replies.length, "unexpected fetch");
            return replies.shift()(options.signal);
        }
    };

    return {
        elements,
        timers,
        requests,
        docEvents,
        winEvents,
        revoked,
        get chimes() { return chimes; },
        get helpers() { return window.QueueDisplay; },
        start() {
            vm.runInNewContext(source, context);
        },
        replyState(state) {
            replies.push(() => Promise.resolve({
                ok: true,
                status: 200,
                json: () => Promise.resolve(state)
            }));
        },
        replyError() {
            replies.push(() => Promise.reject(new Error("network failure")));
        },
        fire(milliseconds) {
            const entries = [...timers].filter(([, t]) => t.milliseconds === milliseconds);
            const entry = entries[entries.length - 1];
            assert.ok(entry, `missing ${milliseconds}ms timer`);
            timers.delete(entry[0]);
            return entry[1].callback();
        }
    };
}

const flush = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };

test("pure diff detection ignores initial state and absent-to-calling transitions", () => {
    const app = setup("screen");
    app.replyState({
        clock: "14:00",
        calling: [],
        absent: [],
        upcoming: [],
        waiting_count: 0,
        front: null
    });
    app.start();
    const { detectNewCalledItems, mergeOverlayNumbers, formatOverlayHeadline } = app.helpers;

    const prev = {
        calling: [{ seq: 10, number: 10, group_size: 2 }],
        absent: [{ seq: 8, number: 8, group_size: 1 }]
    };
    const next = {
        calling: [
            { seq: 8, number: 8, group_size: 1 },   // returned from absent -> NOT new
            { seq: 10, number: 10, group_size: 2 }, // already calling -> NOT new
            { seq: 11, number: 11, group_size: 3 }, // newly called -> new
            { seq: 12, number: 12, group_size: 1 }  // newly called -> new
        ],
        absent: []
    };
    const detected = detectNewCalledItems(prev, next);
    assert.deepEqual(Array.from(detected, (i) => i.number), [11, 12]);

    const merged = mergeOverlayNumbers([11], detected);
    assert.deepEqual(Array.from(merged), [11, 12]);
    assert.equal(formatOverlayHeadline(merged), "No.11 / No.12 の方");
});

test("screen display: initial state does not show overlay; second state shows overlay for new calling seq only, ignoring previously absent seq", async () => {
    const app = setup("screen");
    // 1st state (initial load): seq 10 is calling, seq 8 is absent
    app.replyState({
        clock: "14:05",
        calling: [{ seq: 10, number: 10, group_size: 2 }],
        absent: [{ seq: 8, number: 8, group_size: 3 }],
        upcoming: [{ seq: 11, number: 11, group_size: 1 }],
        waiting_count: 3,
        front: null
    });
    app.start();
    await flush();

    // Activate audio via start button
    app.elements["start-no-video"].listeners.click();
    assert.equal(app.elements["call-overlay"].hidden, true);
    assert.equal(app.elements["screen-clock"].textContent, "14:05");
    assert.equal(app.elements["screen-waiting"].textContent, "待ち 3組");
    assert.equal(app.elements["screen-calling-section"].hidden, false);
    assert.equal(app.elements["screen-upcoming-section"].hidden, false);
    assert.equal(app.elements["screen-absent-section"].hidden, false);
    assert.equal(app.elements["screen-calling"].textContent, "10");
    assert.equal(app.elements["screen-upcoming"].textContent, "11");
    assert.equal(app.elements["screen-absent"].textContent, "8");

    // 2nd state: seq 8 returned from absent to calling (must NOT trigger overlay); absent is now empty -> section hidden
    app.replyState({
        clock: "14:05",
        calling: [
            { seq: 8, number: 8, group_size: 3 },
            { seq: 10, number: 10, group_size: 2 }
        ],
        absent: [],
        upcoming: [{ seq: 11, number: 11, group_size: 1 }],
        waiting_count: 3,
        front: null
    });
    await app.fire(3000);
    await flush();
    assert.equal(app.elements["call-overlay"].hidden, true);
    assert.equal(app.elements["screen-calling-section"].hidden, false);
    assert.equal(app.elements["screen-upcoming-section"].hidden, false);
    assert.equal(app.elements["screen-absent-section"].hidden, true);
    assert.equal(app.elements["screen-absent"].textContent, "");

    // 3rd state: seq 11 is newly called -> overlay appears with No.11
    app.replyState({
        clock: "14:06",
        calling: [
            { seq: 8, number: 8, group_size: 3 },
            { seq: 10, number: 10, group_size: 2 },
            { seq: 11, number: 11, group_size: 1 }
        ],
        absent: [],
        upcoming: [{ seq: 12, number: 12, group_size: 4 }],
        waiting_count: 2,
        front: null
    });
    await app.fire(3000);
    await flush();
    assert.equal(app.elements["call-overlay"].hidden, false);
    assert.equal(app.elements["overlay-numbers"].textContent, "No.11 の方");
    assert.equal(app.elements["pv-video"].volume, 0.2);
    assert.equal(app.chimes, 2); // 2-tone chime

    // 4th state while overlay is active: seq 12 is also called -> appended and extended; upcoming is now empty -> section hidden
    app.replyState({
        clock: "14:06",
        calling: [
            { seq: 8, number: 8, group_size: 3 },
            { seq: 10, number: 10, group_size: 2 },
            { seq: 11, number: 11, group_size: 1 },
            { seq: 12, number: 12, group_size: 4 }
        ],
        absent: [],
        upcoming: [],
        waiting_count: 1,
        front: null
    });
    await app.fire(3000);
    await flush();
    assert.equal(app.elements["call-overlay"].hidden, false);
    assert.equal(app.elements["overlay-numbers"].textContent, "No.11 / No.12 の方");
    assert.equal(app.elements["screen-upcoming-section"].hidden, true);
    assert.equal(app.elements["screen-upcoming"].textContent, "");

    // After 8000ms timer, overlay hides and volume is restored
    app.fire(8000);
    assert.equal(app.elements["call-overlay"].hidden, true);
    assert.equal(app.elements["pv-video"].volume, 1.0);

    // 5th state: all calling/upcoming/absent are empty -> all 3 sections hidden
    app.replyState({
        clock: "14:07",
        calling: [],
        absent: [],
        upcoming: [],
        waiting_count: 0,
        front: null
    });
    await app.fire(3000);
    await flush();
    assert.equal(app.elements["screen-calling-section"].hidden, true);
    assert.equal(app.elements["screen-upcoming-section"].hidden, true);
    assert.equal(app.elements["screen-absent-section"].hidden, true);
    assert.equal(app.elements["screen-calling"].textContent, "");
});

test("front display and fetch failure: retains previous content and shows connecting indicator", async () => {
    const app = setup("front");
    app.replyState({
        clock: "14:10",
        calling: [{ seq: 31, number: 12, group_size: 2 }],
        absent: [{ seq: 28, number: 9, group_size: 3 }],
        upcoming: [],
        waiting_count: 5,
        front: {
            seq: 45,
            number: 26,
            group_size: 4,
            qr_svg: "<svg>qr26</svg>",
            phase: "showing"
        }
    });
    app.start();
    await flush();

    assert.equal(app.elements["front-ticket-view"].hidden, false);
    assert.equal(app.elements["front-idle-view"].hidden, true);
    assert.equal(app.elements["front-number"].textContent, "26");
    assert.equal(app.elements["front-group"].textContent, "4名様");
    assert.equal(app.elements["front-qr"].innerHTML, "<svg>qr26</svg>");
    assert.equal(app.elements["front-viewed"].hidden, true);
    assert.equal(app.elements["display-conn-status"].hidden, true);

    // Fetch fails on next poll -> previous display stays intact and connection status appears
    app.replyError();
    await app.fire(2000);
    await flush();
    assert.equal(app.elements["front-ticket-view"].hidden, false);
    assert.equal(app.elements["front-number"].textContent, "26");
    assert.equal(app.elements["front-qr"].innerHTML, "<svg>qr26</svg>");
    assert.equal(app.elements["display-conn-status"].hidden, false);
    assert.equal(app.elements["display-conn-status"].textContent, "⚠ 接続中…");

    // Next poll succeeds with viewed phase -> connection indicator hides and viewed badge shows
    app.replyState({
        clock: "14:10",
        calling: [{ seq: 31, number: 12, group_size: 2 }],
        absent: [{ seq: 28, number: 9, group_size: 3 }],
        upcoming: [],
        waiting_count: 5,
        front: {
            seq: 45,
            number: 26,
            group_size: 4,
            qr_svg: "<svg>qr26</svg>",
            phase: "viewed"
        }
    });
    await app.fire(2000);
    await flush();
    assert.equal(app.elements["display-conn-status"].hidden, true);
    assert.equal(app.elements["front-viewed"].hidden, false);

    // Next poll returns front: null -> switches to idle view
    app.replyState({
        clock: "14:11",
        calling: [{ seq: 31, number: 12, group_size: 2 }],
        absent: [{ seq: 28, number: 9, group_size: 3 }],
        upcoming: [],
        waiting_count: 5,
        front: null
    });
    await app.fire(2000);
    await flush();
    assert.equal(app.elements["front-ticket-view"].hidden, true);
    assert.equal(app.elements["front-idle-view"].hidden, false);
    assert.equal(app.elements["front-waiting"].textContent, "現在の待ち 5組");
    assert.equal(app.elements["front-calling"].hidden, false);
    assert.equal(app.elements["front-calling"].textContent, "お呼び出し中: 12");
    assert.equal(app.elements["front-absent"].hidden, false);
    assert.match(app.elements["front-absent"].textContent, /お呼びした番号: 9/);

    // Next poll returns empty calling and absent -> both rows are hidden
    app.replyState({
        clock: "14:12",
        calling: [],
        absent: [],
        upcoming: [],
        waiting_count: 4,
        front: null
    });
    await app.fire(2000);
    await flush();
    assert.equal(app.elements["front-waiting"].textContent, "現在の待ち 4組");
    assert.equal(app.elements["front-calling"].hidden, true);
    assert.equal(app.elements["front-calling"].textContent, "");
    assert.equal(app.elements["front-absent"].hidden, true);
    assert.equal(app.elements["front-absent"].textContent, "");
});

function emptyState() {
    return {
        clock: "14:00",
        calling: [],
        absent: [],
        upcoming: [],
        waiting_count: 0,
        front: null
    };
}

function startScreenApp(files) {
    const app = setup("screen");
    app.replyState(emptyState());
    app.start();
    app.elements["pv-file"].files = files;
    app.elements["start-with-video"].listeners.click();
    return app;
}

test("PV playlist: three files sorted by name and looped via ended", () => {
    const app = startScreenApp([
        { name: "10_c.mp4" },
        { name: "2_b.mp4" },
        { name: "1_a.mp4" }
    ]);
    const video = app.elements["pv-video"];
    assert.equal(video.loop, false);
    assert.match(video.src, /1_a/);

    video.listeners.ended();
    assert.match(video.src, /2_b/);
    video.listeners.ended();
    assert.match(video.src, /10_c/);
    video.listeners.ended();
    assert.match(video.src, /1_a/);
});

test("PV playlist: single file keeps loop=true and ignores ended", () => {
    const app = startScreenApp([{ name: "solo.mp4" }]);
    const video = app.elements["pv-video"];
    assert.equal(video.loop, true);
    assert.match(video.src, /solo/);
    video.listeners.ended();
    assert.match(video.src, /solo/);
});

test("PV playlist: restarting revokes previous object URLs", () => {
    const app = startScreenApp([{ name: "1_a.mp4" }, { name: "2_b.mp4" }]);
    assert.match(app.elements["pv-video"].src, /1_a/);

    app.elements["start-with-video"].listeners.click();
    assert.ok(app.revoked.includes("blob:1_a.mp4"));
    assert.ok(app.revoked.includes("blob:2_b.mp4"));
});

test("PV playlist: error skips to next and stops once all videos failed consecutively", () => {
    const app = startScreenApp([{ name: "1_a.mp4" }, { name: "2_b.mp4" }]);
    const video = app.elements["pv-video"];
    assert.match(video.src, /1_a/);

    video.listeners.error();
    assert.match(video.src, /2_b/);
    video.listeners.error();
    assert.match(video.src, /2_b/);
    video.listeners.error();
    assert.match(video.src, /2_b/);
});

test("PV playlist: no-video start clears playlist and ignores ended/error", () => {
    const app = startScreenApp([{ name: "1_a.mp4" }]);
    const video = app.elements["pv-video"];
    const src = video.src;

    app.elements["start-no-video"].listeners.click();
    assert.ok(app.revoked.includes("blob:1_a.mp4"));
    video.listeners.ended();
    video.listeners.error();
    assert.equal(video.src, src);
});

test("PV playlist: overlay volume 0.2 and mute setting survive video switch", async () => {
    const app = startScreenApp([{ name: "1_a.mp4" }, { name: "2_b.mp4" }]);
    const video = app.elements["pv-video"];
    await flush();

    app.replyState({
        clock: "14:01",
        calling: [{ seq: 99, number: 99, group_size: 1 }],
        absent: [],
        upcoming: [],
        waiting_count: 1,
        front: null
    });
    await app.fire(3000);
    await flush();
    assert.equal(app.elements["call-overlay"].hidden, false);
    assert.equal(video.volume, 0.2);

    video.listeners.ended();
    assert.match(video.src, /2_b/);
    assert.equal(video.volume, 0.2);
    assert.equal(video.muted, false);
});
