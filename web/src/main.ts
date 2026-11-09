import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import { app } from './lib/app.svelte';
import { unlockAudio } from './lib/notify';

for (const ev of ['pointerdown', 'keydown'] as const) {
  window.addEventListener(ev, unlockAudio, { once: true, capture: true });
}

void app.start();

export default mount(App, { target: document.getElementById('app')! });
