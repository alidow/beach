import { useEffect, useMemo, useState } from 'react';
import { useBeachManagerSettings } from './BeachSettingsContext';
import { Button } from '../ui/button';
import { Dialog } from '../ui/dialog';
import { Input } from '../ui/input';

export function BeachSettingsButton() {
  const { manager, updateManager, saving } = useBeachManagerSettings();
  const [open, setOpen] = useState(false);
  const [draftManagerUrl, setDraftManagerUrl] = useState(manager.managerUrl);
  const [draftRoadUrl, setDraftRoadUrl] = useState(manager.roadUrl);
  const [draftToken, setDraftToken] = useState(manager.token);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setDraftManagerUrl(manager.managerUrl);
    setDraftRoadUrl(manager.roadUrl);
    setDraftToken(manager.token);
    setError(null);
  }, [open, manager.managerUrl, manager.roadUrl, manager.token]);

  const missingToken = useMemo(() => !manager.token || manager.token.trim().length === 0, [manager.token]);

  const save = async () => {
    setError(null);
    try {
      await updateManager({
        managerUrl: draftManagerUrl.trim(),
        roadUrl: draftRoadUrl.trim(),
        token: draftToken.trim(),
      });
      setOpen(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <>
      <Button
        size="sm"
        variant="ghost"
        onClick={() => setOpen(true)}
        className={missingToken ? 'relative text-amber-600 hover:text-amber-700' : undefined}
      >
        Settings
        {missingToken && (
          <span className="ml-2 rounded-full bg-amber-500/90 px-2 py-[2px] text-[10px] font-semibold text-white">
            Token?
          </span>
        )}
      </Button>
      <Dialog
        open={open}
        onOpenChange={setOpen}
        title="Private Beach Settings"
        description="Update the manager connection details used for dashboards."
        footer={
          <div className="flex items-center justify-end gap-2">
            <Button size="sm" variant="ghost" onClick={() => setOpen(false)}>
              Cancel
            </Button>
            <Button size="sm" onClick={save} disabled={saving}>
              {saving ? 'Saving…' : 'Save'}
            </Button>
          </div>
        }
      >
        <div className="space-y-3">
          <div>
            <label className="mb-1 block text-xs font-medium text-neutral-700">Manager URL</label>
            <Input value={draftManagerUrl} onChange={(e) => setDraftManagerUrl(e.target.value)} placeholder="http://localhost:8080" />
          </div>
          <div>
            <label className="mb-1 block text-xs font-medium text-neutral-700">Road URL (optional)</label>
            <Input value={draftRoadUrl} onChange={(e) => setDraftRoadUrl(e.target.value)} placeholder="https://api.beach.sh" />
            <p className="mt-1 text-[11px] text-neutral-500">Used for fetching your active sessions when adding tiles.</p>
          </div>
          <div>
            <label className="mb-1 block text-xs font-medium text-neutral-700">Manager Token</label>
            <Input value={draftToken} onChange={(e) => setDraftToken(e.target.value)} placeholder="Paste a token with pb:sessions.read scope" />
            <p className="mt-1 text-[11px] text-neutral-500">
              Required for live terminal previews and session events.
            </p>
          </div>
          {error && (
            <div className="rounded border border-red-200 bg-red-50 p-2 text-xs text-red-700">{error}</div>
          )}
        </div>
      </Dialog>
    </>
  );
}
