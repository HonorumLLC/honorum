let cancelCurrent;
export function cancelSolver() { cancelCurrent?.(); }
export function solveSchedule(courses, fixed, options) {
  cancelSolver();
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL('./solver.worker.js', import.meta.url), { type: 'module' });
    let timeout;
    const finish = (error, results) => { clearTimeout(timeout); worker.terminate(); if (cancelCurrent === cancel) cancelCurrent = undefined; error ? reject(error) : resolve(results); };
    const cancel = () => finish(new Error('Arrangement cancelled'));
    cancelCurrent = cancel;
    worker.onmessage = ({ data }) => finish(data.error ? new Error(data.error) : null, data.results);
    worker.onerror = () => finish(new Error('The arrangement worker could not start'));
    timeout = setTimeout(() => finish(new Error('Arrangement took too long. Narrow the schedule and try again.')), 5000);
    worker.postMessage(JSON.parse(JSON.stringify({ courses, fixed, options })));
  });
}
