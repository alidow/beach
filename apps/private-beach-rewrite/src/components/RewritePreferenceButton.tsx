'use client';

import { useCallback } from 'react';
import { rememberPrivateBeachRewritePreference } from '../../../private-beach/src/lib/featureFlags';
import { Button } from '../../../private-beach/src/components/ui/button';

type Props = {
  legacyHref: string;
};

export function RewritePreferenceButton({ legacyHref }: Props) {
  const handleClick = useCallback(() => {
    rememberPrivateBeachRewritePreference(false);
    try {
      const url = new URL(legacyHref, window.location.origin);
      url.searchParams.set('rewrite', '0');
      window.location.href = url.toString();
    } catch {
      window.location.href = `${legacyHref}${legacyHref.includes('?') ? '&' : '?'}rewrite=0`;
    }
  }, [legacyHref]);

  return (
    <Button variant="outline" size="sm" onClick={handleClick}>
      Open legacy
    </Button>
  );
}
