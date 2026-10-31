import { mount } from 'svelte';
import '../lib/theme.css';
import ProgressWindow from './ProgressWindow.svelte';

const app = mount(ProgressWindow, { target: document.getElementById('app')! });

export default app;
