import type { AppProps } from 'next/app';
import { ClerkProvider } from '@clerk/nextjs';
import '../lib/patchReactDraggable';
import '../styles/globals.css';
import '../styles/grid.css';
import 'reactflow/dist/style.css';
import { ThemeProvider } from '../components/theme-provider';
import { ThemeDebugger } from '../components/theme-debugger';

export default function App({ Component, pageProps }: AppProps) {
  return (
    <ClerkProvider {...pageProps}>
      <ThemeProvider>
        <div className="min-h-screen bg-background text-foreground">
          <Component {...pageProps} />
          {process.env.NODE_ENV !== 'production' && <ThemeDebugger />}
        </div>
      </ThemeProvider>
    </ClerkProvider>
  );
}
