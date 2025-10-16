import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import './styles.css';

declare global {
  interface Window {
    __BEACH_TRACE?: boolean;
  }
}

const rootElement = document.getElementById('root');
if (!rootElement) {
  throw new Error('Root element #root not found');
}

if (typeof window !== 'undefined' && window.__BEACH_TRACE) {
  // eslint-disable-next-line no-console
  console.info('[beach-surfer] version', __APP_VERSION__);
}

ReactDOM.createRoot(rootElement).render(<App />);
