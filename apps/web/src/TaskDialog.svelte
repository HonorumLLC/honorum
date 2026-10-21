<script>
  import { untrack } from 'svelte';
  // A task: title, class, due date and time, note. Opens when
  // app.taskEditing holds a task id or 'new'.
  import { app, taskCourses, saveTask, deleteTask, campusZone } from './lib/state.svelte.js';
  import { campusDue, wallInstant } from './lib/tasks.js';
  import { campusNow } from './lib/occurrences.js';
  import { ymd } from './lib/dates.js';
  import { toast } from './lib/ui.svelte.js';
  import Dialog from './Dialog.svelte';
  import Icon from './Icon.svelte';

  let open = $state(false);
  let task = $state({ title: '', course: '', date: '', time: '23:59', note: '' });
  let opened = { date: '', time: '' };
  const isNew = $derived(app.taskEditing === 'new');
  const editing = $derived(app.tasks.find((x) => x.id === app.taskEditing));
  const courses = $derived(taskCourses());
  const pad = (n) => String(n).padStart(2, '0');

  $effect(() => {
    if (!app.taskEditing) return;
    const id = app.taskEditing;
    // Background imports may replace the task while someone is typing.
    const t = untrack(() => app.tasks.find((x) => x.id === id));
    // The date and time are campus wall-clock values, as the task list shows them.
    const due = t?.due ? campusDue(t.due, campusZone()) : null;
    const date = due ? ymd(due) : id === 'new' ? ymd(campusNow(campusZone())) : '', time = due ? `${pad(due.getHours())}:${pad(due.getMinutes())}` : '23:59';
    task = { title: t?.title || '', course: t?.course || '', date, time, note: t?.note || '' };
    opened = { date, time };
    open = true;
  });
  function save() {
    const data = { title: task.title.trim(), course: task.course, note: task.note.trim() };
    // An untouched date is not saved: it would drop the source's seconds and
    // read as the student's own date on the next import.
    if (isNew || task.date !== opened.date || task.time !== opened.time) {
      const [h, m] = (task.time || '23:59').split(':');
      data.due = task.date ? new Date(wallInstant(task.date, +h * 60 + +m, campusZone())).toISOString() : '';
    }
    saveTask(data, isNew ? '' : app.taskEditing);
    open = false;
  }
  function remove() {
    const undo = deleteTask(app.taskEditing);
    open = false;
    toast('Task deleted', { label: 'Undo', run: undo });
  }
</script>

<Dialog bind:open title={isNew ? 'Add Task' : 'Edit Task'} ok={isNew ? 'Add' : 'Save'} okDisabled={!task.title.trim()} onok={save} onclose={() => (app.taskEditing = '')}>
  <!-- svelte-ignore a11y_autofocus -->
  <label class="field"><span>Title</span><input bind:value={task.title} placeholder="What is due?" autofocus></label>
  <label class="field"><span>Class</span>
    <select bind:value={task.course}>
      <option value="">No class</option>
      {#each courses as c (c)}<option value={c}>{c}</option>{/each}
      {#if task.course && !courses.includes(task.course)}<option value={task.course}>{task.course}</option>{/if}
    </select>
  </label>
  <div class="fields">
    <label class="field"><span>Due date</span><input type="date" bind:value={task.date}></label>
    <label class="field"><span>Due time</span><input type="time" bind:value={task.time} disabled={!task.date}></label>
  </div>
  <label class="field"><span>Note</span><input bind:value={task.note} placeholder="Optional"></label>
  {#if editing?.url}
    <div class="list"><a class="row" href={editing.url} target="_blank" rel="noopener"><span class="rt"><span class="t">Open in {editing.source === 'canvas' ? 'Canvas' : editing.source === 'feed' ? 'Calendar' : 'Browser'}</span></span><span class="chev"><Icon name="open" /></span></a></div>
  {/if}
  {#snippet extra()}
    {#if !isNew}<button type="button" class="btn flat delete" onclick={remove}><Icon name="trash" />Delete</button>{/if}
  {/snippet}
</Dialog>

<style>
  .delete { color: var(--red); }
</style>
