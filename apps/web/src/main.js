import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import { toast } from './lib/ui.svelte.js';

mount(App, { target: document.getElementById('app') });
if ('serviceWorker' in navigator && import.meta.env.PROD) {
  navigator.serviceWorker.register('/sw.js').catch(() => toast('Offline access could not be set up. The app still works online.'));
}
