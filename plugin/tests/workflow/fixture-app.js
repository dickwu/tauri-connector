// Isolated, in-memory React business-path fixture shared by browser and Wry
// tests. Load react-fixture.cjs's generated bundle before this file.
(() => {
  const { React, ReactDOM } = window.WorkflowReact;
  const h = React.createElement;
  const stats = { opens: 0, inputEvents: 0, saves: 0, submissions: [], tasks: [], errors: [], backgroundTicks: 0 };
  Object.defineProperty(window, '__workflowFixtureStats', { value: stats });
  Object.defineProperty(window, '__WORKFLOW_FIXTURE__', { value: stats });
  let nextId = 0;
  function App() {
    const [open, setOpen] = React.useState(false);
    const [name, setName] = React.useState('');
    const [tasks, setTasks] = React.useState([]);
    const [detail, setDetail] = React.useState(null);
    const [blocked, setBlocked] = React.useState(false);
    const [entity, setEntity] = React.useState('fixture-entity-a');
    const [background, setBackground] = React.useState(false);
    React.useEffect(() => {
      if (!background) return;
      const timer = setInterval(() => { stats.backgroundTicks++; }, 25);
      return () => clearInterval(timer);
    }, [background]);
    async function submit(event) {
      event.preventDefault();
      stats.saves++;
      stats.submissions.push(name);
      try {
        const invoke = window.__TAURI__?.core?.invoke;
        const task = invoke ? await invoke('fixture_create_task', { name }) : { id: `fixture-task-${++nextId}`, name };
        stats.tasks.push(task);
        setTasks(current => [...current, task]);
        setOpen(false);
      } catch (error) { stats.errors.push(String(error)); }
    }
    return h('main', null,
      h('h1', null, 'Isolated workflow fixture'),
      h('p', null, 'All records are test-only memory. No application backend is connected.'),
      h('button', { onClick() { stats.opens++; setName(''); setOpen(true); } }, '新建任务'),
      open && h('section', { role: 'dialog', 'aria-label': '新建任务', style: { border: '2px solid #666', padding: 16 } },
        h('form', { onSubmit: submit },
          h('label', { htmlFor: 'task-name' }, '名称'),
          h('textarea', { id: 'task-name', value: name, onChange(event) { stats.inputEvents++; setName(event.target.value); } }),
          h('button', { type: 'submit' }, '保存'),
          h('button', { type: 'button', onClick() { setOpen(false); } }, '取消'),
          h('output', { 'data-testid': 'controlled-name' }, name))),
      h('section', { 'aria-label': 'Task list' }, ...tasks.map(task => h('article', {
        key: task.id, 'data-testid': 'task-row', 'data-task-name': task.name, 'data-task-id': task.id
      }, h('span', null, task.name), h('button', { onClick() { setDetail(task); }, 'aria-label': `Details ${task.id}` }, 'Details')))),
      detail && h('aside', { 'data-testid': 'task-detail', 'data-task-id': detail.id }, detail.name),
      h('section', { 'aria-label': 'Adversarial controls', style: { marginTop: 16 } },
        h('button', { 'data-testid': 'duplicate-save' }, '保存'),
        h('button', { 'data-testid': 'duplicate-save' }, '保存'),
        h('button', { 'data-testid': 'disabled-button', disabled: true }, 'Disabled action'),
        h('button', { 'data-testid': 'hidden-button', hidden: true }, 'Hidden action'),
        h('input', { 'aria-label': 'Readonly value', readOnly: true, value: 'fixed' }),
        h('button', { 'data-testid': 'virtual-row', 'data-entity-id': entity }, 'Virtual row'),
        h('button', { onClick() { setEntity(current => current === 'fixture-entity-a' ? 'fixture-entity-b' : 'fixture-entity-a'); } }, 'Recycle row'),
        h('button', { onClick() { setBlocked(true); } }, 'Block controls'),
        h('button', { onClick() { setBackground(current => !current); } }, 'Toggle background activity')),
      blocked && h('div', { 'data-testid': 'blocking-overlay', style: { position: 'fixed', inset: 0, zIndex: 100, background: '#eee9' } },
        h('button', { onClick() { setBlocked(false); } }, 'Dismiss fixture blocker')));
  }
  ReactDOM.createRoot(document.getElementById('fixture') || document.body).render(h(App));
})()
