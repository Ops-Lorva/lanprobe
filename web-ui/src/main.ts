import '@fontsource/geist-sans/400.css';
import '@fontsource/geist-sans/500.css';
import '@fontsource/geist-sans/600.css';
import '@fontsource/geist-sans/700.css';
import '@fontsource/geist-sans/800.css';
import '@fontsource/geist-mono/400.css';
import '@fontsource/geist-mono/500.css';
import '@fontsource/geist-mono/600.css';

// Les tokens de l'app desktop, importés tels quels : une seule définition de la
// palette pour les deux surfaces. Modifier une couleur ici la modifie là-bas.
import '$app/app.css';
import './web.css';

import { mount } from 'svelte';
import App from './App.svelte';
import { initI18n } from '$lib/i18n';
import { theme } from '$lib/theme';

theme.init();
initI18n();

mount(App, { target: document.getElementById('app')! });
