export type ViewerFitMode = 'contain' | 'cover' | 'actual';

export interface MediaStats {
  mode: 'png' | 'h264';
  frames: number;
  width?: number;
  height?: number;
  fps?: number;
  bitrateKbps?: number;
  bufferedSeconds?: number;
  bytes?: number;
  codec?: string | null;
  timestamp: number;
}
