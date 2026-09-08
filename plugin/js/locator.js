(async function connectorLocate(query, connectorInput) {
            const normalize = (value) => String(value || '').replace(/\s+/g, ' ').trim();
            const matchText = (value, expected) => {
                if (expected == null) return true;
                const a = normalize(value);
                const b = normalize(expected);
                return query.exact ? a === b : a.toLowerCase().includes(b.toLowerCase());
            };
            const implicitRole = (el) => {
                const tag = (el.tagName || '').toLowerCase();
                const type = (el.getAttribute('type') || '').toLowerCase();
                if (el.getAttribute('role')) return el.getAttribute('role');
                if (tag === 'button') return 'button';
                if (tag === 'a' && el.hasAttribute('href')) return 'link';
                if (tag === 'select') return 'combobox';
                if (tag === 'textarea') return 'textbox';
                if (tag === 'img') return 'img';
                if (/^h[1-6]$/.test(tag)) return 'heading';
                if (tag === 'input') {
                    if (['button', 'submit', 'reset'].includes(type)) return 'button';
                    if (type === 'checkbox') return 'checkbox';
                    if (type === 'radio') return 'radio';
                    if (type === 'range') return 'slider';
                    return 'textbox';
                }
                return '';
            };
            const labelText = (el) => {
                const id = el.id;
                const labels = [];
                if (el.labels) for (const label of el.labels) labels.push(label.textContent || '');
                if (id) {
                    for (const label of document.querySelectorAll('label[for="' + CSS.escape(id) + '"]')) {
                        labels.push(label.textContent || '');
                    }
                }
                const wrapping = el.closest && el.closest('label');
                if (wrapping) labels.push(wrapping.textContent || '');
                return normalize(labels.join(' '));
            };
            const accessibleName = (el) => {
                const labelledBy = (el.getAttribute('aria-labelledby') || '').split(/\s+/).filter(Boolean)
                    .map((id) => document.getElementById(id)?.textContent || '').join(' ');
                return normalize(
                    el.getAttribute('aria-label') ||
                    labelledBy ||
                    labelText(el) ||
                    el.getAttribute('alt') ||
                    el.getAttribute('title') ||
                    el.getAttribute('placeholder') ||
                    el.textContent ||
                    ''
                );
            };
            const cssPath = (el) => {
                if (!el || el.nodeType !== 1) return '';
                if (el.id) return '#' + CSS.escape(el.id);
                const parts = [];
                let cur = el;
                while (cur && cur.nodeType === 1 && cur !== document.documentElement) {
                    let part = cur.tagName.toLowerCase();
                    const parent = cur.parentElement;
                    if (parent) {
                        const siblings = Array.from(parent.children).filter((child) => child.tagName === cur.tagName);
                        if (siblings.length > 1) part += ':nth-of-type(' + (siblings.indexOf(cur) + 1) + ')';
                    }
                    parts.unshift(part);
                    cur = parent;
                }
                return parts.join(' > ');
            };
            const all = Array.from(document.querySelectorAll('*'));
            const textOnly = ['role', 'label', 'placeholder', 'alt', 'title', 'testId', 'name'].every(key => query[key] == null);
            const filtered = all.filter((el) => {
                if (query.role != null && !matchText(implicitRole(el), query.role)) return false;
                if (query.text != null) {
                    if (!matchText(el.textContent || '', query.text)) return false;
                    if (textOnly && Array.from(el.children || []).some(child => matchText(child.textContent || '', query.text))) return false;
                }
                if (query.label != null && !matchText(labelText(el), query.label)) return false;
                for (const attr of ['placeholder', 'alt', 'title']) {
                    if (query[attr] != null && !matchText(el.getAttribute(attr), query[attr])) return false;
                }
                if (query.testId != null) {
                    const value = el.getAttribute('data-testid') || el.getAttribute('data-test-id') || el.getAttribute('data-test') || el.getAttribute('testid');
                    if (!matchText(value, query.testId)) return false;
                }
                return query.name == null || matchText(accessibleName(el), query.name);
            });
            const count = filtered.length;
            let index = query.last ? count - 1 : 0;
            if (query.nth !== null && query.nth !== undefined) index = Number(query.nth);
            if (query.first) index = 0;
            const el = filtered[index];
            if (!el) return { count, index, code: 'target_not_found', error: 'No element matched locator' };
            const action = query.action || null;
            const value = query.value || '';
            if (action) {
                if (action === 'click') el.click();
                else if (action === 'hover') {
                    const rect = el.getBoundingClientRect();
                    const opts = { bubbles: true, cancelable: true, view: window, clientX: rect.x + rect.width / 2, clientY: rect.y + rect.height / 2 };
                    el.dispatchEvent(new PointerEvent('pointerover', opts));
                    el.dispatchEvent(new MouseEvent('mouseover', opts));
                }
                else if (action === 'focus') el.focus();
                else if (action === 'fill' || action === 'type') {
                    const input = await connectorInput(el, { action, text: value });
                    if (input.error) return { count, index, ...input };
                }
                else if (action === 'check') { if (!el.checked) el.click(); }
                else if (action === 'uncheck') { if (el.checked) el.click(); }
                else if (action !== 'text') return { count, index, code: 'unsupported_feature', error: 'Unsupported locator action: ' + action };
            }
            const rect = el.getBoundingClientRect();
            return {
                count,
                index,
                action,
                interactionMode: action && action !== 'text' ? 'synthetic' : undefined,
                value: action === 'text' ? normalize(el.textContent || '') : undefined,
                matched: {
                    tag: el.tagName.toLowerCase(),
                    role: implicitRole(el) || null,
                    name: accessibleName(el),
                    text: normalize(el.textContent || '').slice(0, 300),
                    selector: cssPath(el),
                    rect: { x: rect.x, y: rect.y, width: rect.width, height: rect.height },
                    visible: !!(rect.width || rect.height || el.getClientRects().length)
                }
            };
})
