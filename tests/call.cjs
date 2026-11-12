// Run with: node tests/call.cjs (no npm packages required).
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");
const source = fs.readFileSync(`${__dirname}/../static/call.js`, "utf8");

function makeElement(id, initial = {}) {
    const children = [];
    const listeners = {};
    const element = {
        id,
        tag: initial.tag || "",
        hidden: Boolean(initial.hidden),
        value: initial.value || "",
        textContent: initial.textContent || "",
        innerHTML: "",
        dataset: initial.dataset || {},
        disabled: false,
        style: {},
        children,
        listeners,
        addEventListener(name, callback) {
            listeners[name] = callback;
        },
        replaceChildren() {
            children.length = 0;
        },
        appendChild(child) {
            children.push(child);
        },
        querySelectorAll(selector) {
            if (selector === "button") {
                return children.filter((child) => child.tag === "button");
            }
            return [];
        },
        closest(selector) {
            if (selector === "[data-partial-open]") {
                return element.dataset.partialOpen !== undefined ? element : null;
            }
            return null;
        }
    };
    return element;
}

function setup() {
    const documentEvents = {};
    const elements = {
        "partial-backdrop": makeElement("partial-backdrop", { hidden: true }),
        "partial-form": makeElement("partial-form"),
        "partial-title": makeElement("partial-title"),
        "partial-subtitle": makeElement("partial-subtitle"),
        "partial-id": makeElement("partial-id"),
        "partial-expected": makeElement("partial-expected"),
        "partial-grid": makeElement("partial-grid"),
        "partial-cancel": makeElement("partial-cancel")
    };
    const body = { style: {} };
    const document = {
        body,
        getElementById: (id) => elements[id] || null,
        createElement: (tag) => makeElement(tag, { tag }),
        addEventListener: (name, callback) => {
            documentEvents[name] = callback;
        }
    };
    vm.runInNewContext(source, { document });
    return { elements, documentEvents, body };
}

function cardButton(number, groupSize, seatedCount) {
    return makeElement("partial-open", {
        dataset: {
            partialOpen: "",
            number: String(number),
            groupSize: String(groupSize),
            seatedCount: String(seatedCount),
            id: "ticket-id-123"
        }
    });
}

test("opening the sheet with 4 remaining builds 1-3 buttons and fills hidden fields", () => {
    const app = setup();
    const opener = cardButton(12, 4, 0);
    app.documentEvents.click({ target: opener });

    assert.equal(app.elements["partial-backdrop"].hidden, false);
    assert.equal(app.elements["partial-title"].textContent, "No.12 何名ご案内しましたか？");
    assert.equal(app.elements["partial-subtitle"].textContent, "4名中 残り4名");
    assert.equal(app.elements["partial-id"].value, "ticket-id-123");
    assert.equal(app.elements["partial-expected"].value, "0");
    assert.equal(app.body.style.overflow, "hidden");

    const buttons = app.elements["partial-grid"].children;
    assert.deepEqual(buttons.map((b) => b.value), ["1", "2", "3"]);
    assert.deepEqual(buttons.map((b) => b.textContent), ["1名", "2名", "3名"]);
    assert.ok(buttons.every((b) => b.type === "submit" && b.name === "seat"));
});

test("opening the sheet with 2 remaining builds only the 1-person button", () => {
    const app = setup();
    app.documentEvents.click({ target: cardButton(7, 4, 2) });

    assert.equal(app.elements["partial-subtitle"].textContent, "4名中 残り2名");
    assert.equal(app.elements["partial-expected"].value, "2");
    const buttons = app.elements["partial-grid"].children;
    assert.deepEqual(buttons.map((b) => b.value), ["1"]);
    assert.equal(buttons[0].textContent, "1名");
});

test("cancel button closes the sheet and restores background scrolling", () => {
    const app = setup();
    app.documentEvents.click({ target: cardButton(12, 4, 0) });
    assert.equal(app.elements["partial-backdrop"].hidden, false);

    app.elements["partial-cancel"].listeners.click();
    assert.equal(app.elements["partial-backdrop"].hidden, true);
    assert.equal(app.body.style.overflow, "");

    // Backdrop click also closes, but clicking inside the sheet does not.
    app.documentEvents.click({ target: cardButton(12, 4, 0) });
    app.elements["partial-backdrop"].listeners.click({
        target: app.elements["partial-sheet"]
    });
    assert.equal(app.elements["partial-backdrop"].hidden, false);
    app.elements["partial-backdrop"].listeners.click({
        target: app.elements["partial-backdrop"]
    });
    assert.equal(app.elements["partial-backdrop"].hidden, true);
});

test("submitting the form disables every seat button to prevent double submission", () => {
    const app = setup();
    app.documentEvents.click({ target: cardButton(12, 4, 0) });
    const buttons = app.elements["partial-grid"].children;
    assert.ok(buttons.every((b) => b.disabled === false));

    app.elements["partial-form"].listeners.submit();
    assert.ok(buttons.every((b) => b.disabled === true));
    assert.equal(app.elements["partial-cancel"].disabled, true);
});
