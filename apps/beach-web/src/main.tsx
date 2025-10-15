import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import './styles.css';

const rootElement = document.getElementById('root');
if (!rootElement) {
  throw new Error('Root element #root not found');
}

if (typeof window !== 'undefined' && (window as any).__BEACH_TRACE) {
  // eslint-disable-next-line no-console
  console.info('[beach-web] version', __APP_VERSION__);
}

ReactDOM.createRoot(rootElement).render(<App />);
