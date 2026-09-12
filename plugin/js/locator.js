(async function connectorLocate(query, connectorInput) {
            const normalize = (value) => String(value || '').replace(/\s+/g, ' ').trim();
            const matchText = (value, expected) => {
                if (expected == null) return true;
                const a = normalize(value);
                const b = normalize(expected);
                return query.exact ? a === b : a.toLowerCase().includes(b.toLowerCase());
            };
            const semantic = window.__CONNECTOR_SEMANTIC__;
            if (!semantic) return {code:'runtime_missing', error:'Trusted semantic runtime is missing'};
            const implicitRole = semantic.getRole;
            const accessibleName = semantic.getAccessibleName;
            const labelText = el => semantic.getLabelTexts(el).join(' ');
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

            const textOnly = ['role', 'label', 'placeholder', 'alt', 'title', 'testId', 'name'].every(key => query[key] == null);
            const findMatches = () => Array.from(document.querySelectorAll('*')).filter((el) => {
                if (el.isConnected === false || semantic.isConnectorOwned(el)) return false;
                if ((query.role != null || query.label != null) && !semantic.isAccessibilityExposed(el)) return false;
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
            const filtered = findMatches();
            const count = filtered.length;
            let index = query.last ? count - 1 : 0;
            if (query.nth !== null && query.nth !== undefined) index = Number(query.nth);
            if (query.first) index = 0;
            const el = filtered[index];
            if (!el) return { count, index, code: 'target_not_found', error: 'No element matched locator' };
            const action = query.action || null;
            const value = query.value || '';
            if (action && action !== 'text' && count > 1 && !query.first && !query.last && query.nth == null) return {count, code:'ambiguous_target', error:'Locator matched multiple elements; provide a unique locator or explicit index'};
            if (action && action !== 'text') {
                if (!semantic.checkActionability(el,action).actionable) return {count,index,code:'not_actionable',error:'Target is not actionable'};
                const current = findMatches();
                if (current.length !== count || current[index] !== el) return {count,index,code:'target_changed',error:'Target changed before dispatch'};
            }
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
                value: action === 'text' ? (semantic.isSensitive(el) ? '[redacted]' : normalize(el.textContent || '')) : undefined,
                matched: {
                    tag: el.tagName.toLowerCase(),
                    role: implicitRole(el) || null,
                    name: semantic.isSensitive(el) ? '[redacted]' : accessibleName(el),
                    description: semantic.isSensitive(el) ? '[redacted]' : semantic.getAccessibleDescription(el),
                    states: semantic.getAriaStates(el),
                    semanticVersion: semantic.version,
                    text: semantic.isSensitive(el) ? '[redacted]' : normalize(el.textContent || '').slice(0, 300),
                    selector: cssPath(el),
                    rect: { x: rect.x, y: rect.y, width: rect.width, height: rect.height },
                    visible: !!(rect.width || rect.height || el.getClientRects().length)
                }
            };
})
