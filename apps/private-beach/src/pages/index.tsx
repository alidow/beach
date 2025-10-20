import { useRouter } from 'next/router';
import { useEffect } from 'react';
import { useAuth } from '@clerk/nextjs';

export default function Home() {
  const router = useRouter();
  const { isLoaded, isSignedIn } = useAuth();

  useEffect(() => {
    if (!isLoaded) return;
    if (isSignedIn) {
      router.replace('/beaches');
    } else {
      router.replace('/sign-in');
    }
  }, [isLoaded, isSignedIn, router]);

  return (
    <div className="flex min-h-screen items-center justify-center text-sm text-muted-foreground">
      Loading…
    </div>
  );
}
