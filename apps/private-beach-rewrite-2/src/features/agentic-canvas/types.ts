export type AgentNodeData = {
  id: string;
  label: string;
  role: string;
  responsibility: string;
  isEditing: boolean;
  onSave: (payload: { id: string; role: string; responsibility: string }) => void;
  onCancel: (payload: { id: string }) => void;
  onEdit: (payload: { id: string }) => void;
};

export type ApplicationNodeData = {
  id: string;
  label: string;
  description: string;
  isEditing: boolean;
  onSave: (payload: { id: string; label: string; description: string }) => void;
  onCancel: (payload: { id: string }) => void;
  onEdit: (payload: { id: string }) => void;
};

export type UpdateMode = 'idle-summary' | 'push' | 'poll';

export type AssignmentEdgeData = {
  instructions: string;
  updateMode: UpdateMode;
  pollFrequency: number;
  isEditing: boolean;
  onSave: (
    payload: {
      id: string;
      instructions: string;
      updateMode: UpdateMode;
      pollFrequency: number;
    },
  ) => void;
  onEdit: (payload: { id: string }) => void;
  onDelete: (payload: { id: string }) => void;
};
