import { generate } from './schedule.js';
self.onmessage = ({ data }) => {
  try { self.postMessage({ results: generate(data.courses, data.fixed, data.options) }); }
  catch (error) { self.postMessage({ error: error.message }); }
};
