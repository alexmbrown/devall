import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import './styles/tokens.css';
import './styles/app.css';
import './styles/devall.css';
import { App } from './App';
import { ErrorBoundary } from './components/layout/ErrorBoundary';
import { useWorkspace } from './store/workspace';
import { useNav } from './store/nav';
import { usePrefs } from './store/prefs';
import { useWorkbench } from './store/workbench';

// Dev builds expose the stores for end-to-end scripts driving the WebView over CDP.
if (import.meta.env.DEV) Object.assign(window, { __devall: { useWorkspace, useNav, usePrefs, useWorkbench } });

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    {/* Last line of defence: a render error in the shell (top bar, sidebar) must not blank the window. */}
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </StrictMode>,
);
