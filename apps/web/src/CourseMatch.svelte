<script>
  // Match each Canvas course to a class on the schedule, once. Its tasks
  // then take the class's color and filter.
  import { canvasCourses, mapCourse, taskCourses, taskCourseCode } from './lib/state.svelte.js';
  import Icon from './Icon.svelte';

  const rows = $derived(canvasCourses());
  const classes = $derived(taskCourses());
  const current = (c) => taskCourseCode({ course: c.name, courseCode: c.code, courseId: c.id, canvasConnection: c.connection });
</script>

<div class="list">
  {#each rows as c (c.id)}
    <label class="row" title={c.code && c.name && c.name !== c.code ? c.name : ''}>
      <span class="rt"><span class="t">{c.code || c.name}</span></span>
      <span class="combo">
        <select value={current(c)} onchange={(e) => mapCourse(c.id, e.target.value, c.connection)} aria-label="Class for {c.code || c.name}">
          <option value="">None</option>
          {#each classes as k (k)}<option value={k}>{k}</option>{/each}
        </select><Icon name="down" />
      </span>
    </label>
  {/each}
</div>
