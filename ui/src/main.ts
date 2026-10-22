// main.ts - mounts the Svelte app and loads the global theme.
import './app.css';
import { mount } from 'svelte';
import App from './App.svelte';

mount(App, { target: document.getElementById('app')! });
