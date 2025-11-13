import type { RelationshipCadenceConfig, RelationshipUpdateMode } from '../tiles/types';

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

export type AssignmentEdgeData = {
  instructions: string;
  updateMode: RelationshipUpdateMode;
  pollFrequency: number;
  cadence: RelationshipCadenceConfig;
  isEditing: boolean;
  connectionState?: 'pending' | 'slow' | 'fast' | 'error';
  connectionMessage?: string | null;
  onSave: (
    payload: {
      id: string;
      instructions: string;
      updateMode: RelationshipUpdateMode;
      pollFrequency: number;
      cadence: RelationshipCadenceConfig;
    },
  ) => void;
  onEdit: (payload: { id: string }) => void;
  onDelete: (payload: { id: string }) => void;
};
