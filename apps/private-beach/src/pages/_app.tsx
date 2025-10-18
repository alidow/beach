import type { AppProps } from 'next/app';
import { useEffect, useState } from 'react';

export default function App({ Component, pageProps }: AppProps) {
  // Simple CSS reset + light container
  return (
    <div style={{ fontFamily: 'system-ui, sans-serif', color: '#111', background: '#fafafa', minHeight: '100vh' }}>
      <Component {...pageProps} />
    </div>
  );
}

