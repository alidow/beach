import { ClerkProvider } from '@clerk/nextjs';
import type { Metadata } from 'next';
import { ThemeProvider } from '../../../private-beach/src/components/theme-provider';
import { ThemeDebugger } from '../../../private-beach/src/components/theme-debugger';
import '@/lib/ensureReactCache';
import './globals.css';

export const metadata: Metadata = {
  title: 'Private Beach Rewrite',
  description: 'Simplified canvas experience for Private Beach.',
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <ClerkProvider>
      <html lang="en" suppressHydrationWarning>
        <body className="min-h-screen bg-background text-foreground antialiased">
          <ThemeProvider attribute="class" defaultTheme="system" enableSystem>
            {children}
            {process.env.NODE_ENV !== 'production' && <ThemeDebugger />}
          </ThemeProvider>
        </body>
      </html>
    </ClerkProvider>
  );
}
