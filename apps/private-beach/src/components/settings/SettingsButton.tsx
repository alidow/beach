import { useEffect, useState } from 'react';
import { useBeachManagerSettings } from './BeachSettingsContext';
import { Button } from '../ui/button';
import { Dialog } from '../ui/dialog';
import { Input } from '../ui/input';

export function BeachSettingsButton() {
  const { manager, updateManager, saving } = useBeachManagerSettings();
  const [open, setOpen] = useState(false);
  const [draftManagerUrl, setDraftManagerUrl] = useState(manager.managerUrl);
  const [draftRoadUrl, setDraftRoadUrl] = useState(manager.roadUrl);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setDraftManagerUrl(manager.managerUrl);
    setDraftRoadUrl(manager.roadUrl);
    setError(null);
  }, [open, manager.managerUrl, manager.roadUrl]);

  const save = async () => {
    setError(null);
    try {
      await updateManager({
        managerUrl: draftManagerUrl.trim(),
        roadUrl: draftRoadUrl.trim(),
      });
      setOpen(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <>
      <Button size="sm" variant="ghost" onClick={() => setOpen(true)}>
        Settings
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
            <label className="mb-1 block text-xs font-medium text-muted-foreground">Manager URL</label>
            <Input value={draftManagerUrl} onChange={(e) => setDraftManagerUrl(e.target.value)} placeholder="http://localhost:8080" />
          </div>
          <div>
            <label className="mb-1 block text-xs font-medium text-muted-foreground">Road URL (optional)</label>
            <Input value={draftRoadUrl} onChange={(e) => setDraftRoadUrl(e.target.value)} placeholder="https://api.beach.sh" />
            <p className="mt-1 text-[11px] text-muted-foreground">Used for fetching your active sessions when adding tiles.</p>
          </div>
          {error && (
            <div className="rounded border border-red-500/40 bg-red-500/10 p-2 text-xs text-red-600 dark:text-red-400">{error}</div>
          )}
        </div>
      </Dialog>
    </>
  );
}
