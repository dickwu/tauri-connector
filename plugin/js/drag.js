// Drags el to (endX, endY) with HTML5 drag events or a pointer and mouse
// sequence. Shared by webview_interact action:drag and the CLI drag command.
(function connectorDrag(el, endX, endY, options) {
  return new Promise((resolve) => {
    const srcRect = el.getBoundingClientRect();
    const startX = srcRect.x + srcRect.width / 2;
    const startY = srcRect.y + srcRect.height / 2;
    const dragStrat = options.dragStrategy;
    const useHtml5 = dragStrat === 'html5dnd' || (dragStrat === 'auto' && (el.draggable === true || el.getAttribute('draggable') === 'true'));
    const totalSteps = Math.max(1, options.steps | 0);
    const stepDelay = options.durationMs / totalSteps;
    const opts = { bubbles: true, cancelable: true, view: window };
    if (useHtml5) {
        const dt = new DataTransfer();
        el.dispatchEvent(new DragEvent('dragstart', { ...opts, dataTransfer: dt, clientX: startX, clientY: startY }));
        let step = 0;
        const doStep = () => {
            step++;
            const t = step / totalSteps;
            const mx = startX + (endX - startX) * t;
            const my = startY + (endY - startY) * t;
            const cur = document.elementFromPoint(mx, my) || document.body;
            cur.dispatchEvent(new DragEvent('dragenter', { ...opts, dataTransfer: dt, clientX: mx, clientY: my }));
            cur.dispatchEvent(new DragEvent('dragover', { ...opts, dataTransfer: dt, clientX: mx, clientY: my }));
            if (step < totalSteps) {
                setTimeout(doStep, stepDelay);
            } else {
                const dropEl = document.elementFromPoint(endX, endY) || document.body;
                dropEl.dispatchEvent(new DragEvent('drop', { ...opts, dataTransfer: dt, clientX: endX, clientY: endY }));
                el.dispatchEvent(new DragEvent('dragend', { ...opts, dataTransfer: dt, clientX: endX, clientY: endY }));
                resolve({ action: 'drag', strategy: 'html5dnd', from: { x: startX, y: startY }, to: { x: endX, y: endY }, steps: totalSteps, sourceTag: el.tagName.toLowerCase(), targetTag: dropEl.tagName.toLowerCase() });
            }
        };
        setTimeout(doStep, stepDelay);
    } else {
        // The primary button is held from down to up: buttons=1 on down and
        // move, 0 after release, as a real mouse reports it.
        const ptr = { pointerId: 1, pointerType: 'mouse', isPrimary: true };
        el.dispatchEvent(new PointerEvent('pointerdown', { ...opts, ...ptr, clientX: startX, clientY: startY, button: 0, buttons: 1 }));
        el.dispatchEvent(new MouseEvent('mousedown', { ...opts, clientX: startX, clientY: startY, button: 0, buttons: 1 }));
        let step = 0;
        const doStep = () => {
            step++;
            const t = step / totalSteps;
            const mx = startX + (endX - startX) * t;
            const my = startY + (endY - startY) * t;
            const cur = document.elementFromPoint(mx, my) || document.body;
            cur.dispatchEvent(new PointerEvent('pointermove', { ...opts, ...ptr, clientX: mx, clientY: my, button: -1, buttons: 1 }));
            cur.dispatchEvent(new MouseEvent('mousemove', { ...opts, clientX: mx, clientY: my, button: 0, buttons: 1 }));
            if (step < totalSteps) {
                setTimeout(doStep, stepDelay);
            } else {
                const dropEl = document.elementFromPoint(endX, endY) || document.body;
                dropEl.dispatchEvent(new PointerEvent('pointerup', { ...opts, ...ptr, clientX: endX, clientY: endY, button: 0, buttons: 0 }));
                dropEl.dispatchEvent(new MouseEvent('mouseup', { ...opts, clientX: endX, clientY: endY, button: 0, buttons: 0 }));
                resolve({ action: 'drag', strategy: 'pointer', from: { x: startX, y: startY }, to: { x: endX, y: endY }, steps: totalSteps, sourceTag: el.tagName.toLowerCase(), targetTag: dropEl.tagName.toLowerCase() });
            }
        };
        setTimeout(doStep, stepDelay);
    }
  });
})
