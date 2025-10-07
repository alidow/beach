import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import AppV2 from './AppV2';
import { isTerminalFirstShell } from './lib/featureFlags';
import './styles.css';

const rootElement = document.getElementById('root');
if (!rootElement) {
  throw new Error('Root element #root not found');
}

const RootComponent = isTerminalFirstShell() ? AppV2 : App;

ReactDOM.createRoot(rootElement).render(
  <RootComponent />,
);
