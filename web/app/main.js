import { render } from 'preact';
import { html } from 'htm/preact';
import { watchTheme } from './theme.js';
import { connect } from './store.js';
import { App, installFocusModel } from './ui/app.js';

watchTheme();
installFocusModel();
const root = document.getElementById('app');
root.replaceChildren();
render(html`<${App} />`, root);
connect();
