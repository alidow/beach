import { useEffect } from 'react';

interface UseDocumentTitleOptions {
  sessionId?: string;
}

export function useDocumentTitle({ sessionId }: UseDocumentTitleOptions): void {
  useEffect(() => {
    const previous = document.title;
    const trimmed = sessionId?.trim();
    const nextSuffix = trimmed && trimmed.length > 0 ? ` - ${trimmed}` : '';
    document.title = `Beach${nextSuffix}`;
    return () => {
      document.title = previous;
    };
  }, [sessionId]);
}
