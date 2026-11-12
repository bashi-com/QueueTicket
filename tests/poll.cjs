// Run with: node tests/poll.cjs (no npm packages required).
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");
const source = fs.readFileSync(`${__dirname}/../static/poll.js`, "utf8");

function setup({ interval = 15, status = "waiting" } = {}) {
    const timers = new Map();
    const intervals = new Map();
    const events = {};
    const windowEvents = {};
    const replies = [];
    const requests = [];
    const vibrations = [];
    let timerId = 0;
    let beeps = 0;
    let click;
    const updated = {};
    const banner = { hidden: true };
    const returnError = { hidden: true, textContent: "" };
    const returnButton = { disabled: false, textContent: "🙋 会場に戻りました" };
    const returnForm = {
        action: "/guest/123/return",
        dataset: { returnForm: "" },
        querySelector: (selector) => selector === "button" ? returnButton : null
    };
    const elementEvents = {};
    const button = { addEventListener: (_, callback) => { click = callback; } };
    const element = {
        id: "ticket",
        dataset: { pollUrl: "/content", pollInterval: String(interval) },
        innerHTML: `<div data-status="${status}">initial content</div>`,
        addEventListener(name, callback) {
            elementEvents[name] = callback;
        },
        querySelector(selector) {
            if (selector === "[data-return-error]") return returnError;
            const match = this.innerHTML.match(/data-status="([^"]+)"/);
            return match ? { dataset: { status: match[1] } } : null;
        }
    };
    const document = {
        title: "マイページ", hidden: false,
        getElementById: () => button,
        querySelectorAll: () => [element],
        querySelector: (selector) => selector.includes("updated") ? updated : banner,
        addEventListener: (name, callback) => { events[name] = callback; }
    };
    class AudioContext {
        currentTime = 0;
        resume() { return Promise.resolve(); }
        createOscillator() {
            return { frequency: {}, connect() {}, start() { beeps++; }, stop() {} };
        }
        createGain() {
            return { connect() {}, gain: { setValueAtTime() {}, exponentialRampToValueAtTime() {} } };
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
        setInterval(callback, milliseconds) {
            const id = ++timerId;
            intervals.set(id, { callback, milliseconds });
            return id;
        },
        clearInterval: (id) => intervals.delete(id),
        addEventListener: (name, callback) => { windowEvents[name] = callback; }
    };
    vm.runInNewContext(source, {
        document, window, AbortController,
        navigator: { vibrate: (value) => vibrations.push(Array.from(value)) },
        fetch(url, options) {
            requests.push({ url, options });
            assert.ok(replies.length, "unexpected fetch");
            return replies.shift()(options.signal);
        }
    });
    return {
        element, updated, banner, timers, intervals, document, requests, vibrations,
        events, windowEvents, returnError, returnButton, returnForm,
        get beeps() { return beeps; },
        click: () => click(),
        submitReturn() {
            let prevented = false;
            const promise = elementEvents.submit({
                target: returnForm,
                preventDefault() { prevented = true; }
            });
            return { promise, get prevented() { return prevented; } };
        },
        returnStatus(status) {
            replies.push(() => Promise.resolve({ ok: status >= 200 && status < 300, status }));
        },
        success(status, seconds) {
            replies.push(() => Promise.resolve({
                ok: true,
                text: () => Promise.resolve(`<div data-status="${status}">updated content</div>`),
                headers: { get: () => String(seconds) }
            }));
        },
        failure() { replies.push(() => Promise.reject(new Error("offline"))); },
        httpFailure() { replies.push(() => Promise.resolve({ ok: false, status: 503 })); },
        hang() {
            replies.push((signal) => new Promise((_, reject) => {
                signal.addEventListener("abort", () => reject(new Error("aborted")));
            }));
        },
        fire(milliseconds) {
            const entry = [...timers].find(([, timer]) => timer.milliseconds === milliseconds);
            assert.ok(entry, `missing ${milliseconds}ms timer`);
            timers.delete(entry[0]);
            return entry[1].callback();
        }
    };
}

const flush = async () => { for (let n = 0; n < 8; n++) await Promise.resolve(); };

test("failed requests retain content and back off 15/30/60, then recover", async () => {
    const app = setup();
    const initial = app.element.innerHTML;
    assert.match(app.updated.textContent, /^最終更新 \d{1,2}:\d{2}:\d{2}$/);
    for (const delay of [15000, 15000, 30000, 60000]) {
        app.failure();
        await app.fire(delay);
        assert.equal(app.element.innerHTML, initial);
        assert.equal(app.banner.hidden, false);
        assert.match(app.banner.textContent, /^⚠ 接続できません（最終更新 .+）。再接続を試みています…$/);
    }
    app.success("waiting", 30);
    await app.fire(60000);
    assert.equal(app.banner.hidden, true);
    assert.match(app.element.innerHTML, /updated content/);
    assert.deepEqual([...app.timers.values()].map((t) => t.milliseconds), [30000]);
    assert.equal(app.requests[0].options.cache, "no-store");
});

test("non-2xx responses retain the current ticket", async () => {
    const app = setup();
    const initial = app.element.innerHTML;
    app.httpFailure();
    await app.fire(15000);
    assert.equal(app.element.innerHTML, initial);
    assert.equal(app.banner.hidden, false);
});

test("10-second timeout aborts fetch and starts retry", async () => {
    const app = setup();
    app.hang();
    const pending = app.fire(15000);
    app.fire(10000);
    await pending;
    assert.equal(app.requests[0].options.signal.aborted, true);
    assert.equal(app.banner.hidden, false);
    assert.deepEqual([...app.timers.values()].map((t) => t.milliseconds), [15000]);
});

test("hidden pages stop polling and abort in-flight requests; visible and online fetch immediately", async () => {
    const app = setup();
    app.hang();
    const pending = app.fire(15000);
    app.document.hidden = true;
    app.events.visibilitychange();
    await pending;
    assert.equal(app.requests[0].options.signal.aborted, true);
    assert.equal(app.timers.size, 0);
    assert.equal(app.banner.hidden, true);
    app.success("waiting", 60);
    app.document.hidden = false;
    app.events.visibilitychange();
    await flush();
    assert.equal(app.requests.length, 2);
    assert.deepEqual([...app.timers.values()].map((t) => t.milliseconds), [60000]);
    app.success("waiting", 30);
    app.windowEvents.online();
    await flush();
    assert.equal(app.requests.length, 3);
    assert.deepEqual([...app.timers.values()].map((t) => t.milliseconds), [30000]);
});

test("waiting to called vibrates and flashes title; sound requires button; completed stops", async () => {
    const app = setup();
    app.success("called", 15);
    await app.fire(15000);
    assert.deepEqual(app.vibrations, [[300, 100, 300]]);
    assert.equal(app.document.title, "【呼出中】");
    assert.equal(app.intervals.size, 1);
    assert.equal(app.beeps, 0);
    app.success("called", 15);
    await app.fire(15000);
    assert.equal(app.vibrations.length, 1);
    app.success("completed", 0);
    await app.fire(15000);
    assert.equal(app.timers.size, 0);
    assert.equal(app.intervals.size, 0);
    assert.equal(app.document.title, "マイページ");
    app.windowEvents.online();
    app.events.visibilitychange();
    assert.equal(app.requests.length, 3);

    const soundApp = setup();
    await soundApp.click();
    soundApp.success("called", 15);
    await soundApp.fire(15000);
    assert.equal(soundApp.beeps, 1);
});

test("an initially completed ticket does not start polling", () => {
    const app = setup({ interval: 0, status: "completed" });
    assert.equal(app.timers.size, 0);
    app.windowEvents.online();
    app.events.visibilitychange();
    assert.equal(app.requests.length, 0);
});

test("return form submit sends fetch POST, refreshes on 204/409, and restores button on error", async () => {
    const app = setup({ interval: 15, status: "called" });
    app.returnStatus(204);
    app.success("called", 15);
    const submission = app.submitReturn();
    assert.equal(submission.prevented, true);
    assert.equal(app.returnButton.disabled, true);
    assert.equal(app.returnButton.textContent, "送信中…");
    await submission.promise;
    assert.equal(app.requests.length, 2);
    assert.equal(app.requests[0].url, "/guest/123/return");
    assert.equal(app.requests[0].options.method, "POST");
    assert.equal(app.requests[0].options.headers["X-Requested-With"], "fetch");
    assert.equal(app.requests[0].options.cache, "no-store");
    assert.equal(app.requests[1].url, "/content");
    assert.match(app.element.innerHTML, /updated content/);

    // Reset button for failure scenario
    app.returnButton.disabled = false;
    app.returnButton.textContent = "🙋 会場に戻りました";
    app.failure();
    const failed = app.submitReturn();
    await failed.promise;
    assert.equal(app.returnError.hidden, false);
    assert.equal(app.returnError.textContent, "送信できませんでした。スタッフにこの画面を見せてください");
    assert.equal(app.returnButton.disabled, false);
    assert.equal(app.returnButton.textContent, "🙋 会場に戻りました");
});
