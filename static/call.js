(() => {
    "use strict";

    const backdrop = document.getElementById("partial-backdrop");
    const form = document.getElementById("partial-form");
    const title = document.getElementById("partial-title");
    const subtitle = document.getElementById("partial-subtitle");
    const idInput = document.getElementById("partial-id");
    const expectedInput = document.getElementById("partial-expected");
    const grid = document.getElementById("partial-grid");
    const cancelButton = document.getElementById("partial-cancel");
    // Disabled buttons are excluded from form data, so the chosen seat count is
    // copied into this hidden field before the buttons are disabled.
    const seatInput = document.getElementById("partial-seat-value");

    if (!backdrop || !form || !grid) return;

    let isOpen = false;

    function closeSheet() {
        if (!isOpen) return;
        isOpen = false;
        backdrop.hidden = true;
        document.body.style.overflow = "";
    }

    function openSheet(opener) {
        const data = opener.dataset || {};
        const number = data.number || "";
        const groupSize = Number(data.groupSize) || 0;
        const seatedCount = Number(data.seatedCount) || 0;
        const remaining = Math.max(0, groupSize - seatedCount);

        if (idInput) idInput.value = data.id || "";
        if (expectedInput) expectedInput.value = String(seatedCount);
        if (seatInput) seatInput.value = "";
        if (title) title.textContent = `No.${number} 何名ご案内しましたか？`;
        if (subtitle) subtitle.textContent = `${groupSize}名中 残り${remaining}名`;

        if (typeof grid.replaceChildren === "function") {
            grid.replaceChildren();
        } else {
            grid.innerHTML = "";
        }
        for (let seat = 1; seat < remaining; seat++) {
            const button = document.createElement("button");
            button.type = "submit";
            button.name = "seat";
            button.value = String(seat);
            button.className = "partial-seat";
            button.textContent = `${seat}名`;
            grid.appendChild(button);
        }

        if (cancelButton) cancelButton.disabled = false;
        backdrop.hidden = false;
        document.body.style.overflow = "hidden";
        isOpen = true;
    }

    document.addEventListener("click", (event) => {
        const target = event.target;
        const opener = target && typeof target.closest === "function"
            ? target.closest("[data-partial-open]")
            : null;
        if (opener) openSheet(opener);
    });

    backdrop.addEventListener("click", (event) => {
        if (event.target === backdrop) closeSheet();
    });
    if (cancelButton) cancelButton.addEventListener("click", closeSheet);
    document.addEventListener("keydown", (event) => {
        if (event.key === "Escape") closeSheet();
    });

    grid.addEventListener("click", (event) => {
        const target = event.target;
        if (seatInput && target && target.name === "seat") seatInput.value = target.value;
    });

    form.addEventListener("submit", (event) => {
        const submitter = event && event.submitter;
        if (seatInput && submitter && submitter.name === "seat") seatInput.value = submitter.value;
        const buttons = grid.querySelectorAll("button");
        for (const button of buttons) button.disabled = true;
        if (cancelButton) cancelButton.disabled = true;
    });
})();
