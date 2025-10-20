import type { AppProps } from 'next/app';
import '../lib/patchReactDraggable';
import '../styles/globals.css';
import '../styles/grid.css';

export default function App({ Component, pageProps }: AppProps) {
  // Simple CSS reset + light container
  return (
    <div className="min-h-screen bg-[rgb(var(--bg))] text-[rgb(var(--fg))]">
      <Component {...pageProps} />
    </div>
  );
}
