export type View = 'storage' | 'cleanup' | 'settings';
export type ProviderMode = 'disabled' | 'openai' | 'typesafe' | 'custom';
export type Sort = 'size' | 'name' | 'modified';
export type ScanSummary = {
  id: number;
  root: string;
  rootId: number;
  status: string;
  entries: number;
  logicalBytes: number;
  allocatedBytes: number;
  allocatedEstimated: boolean;
  candidateBytes: number;
  errors: number;
  skipped: number;
  startedAt: number;
  finishedAt: number | null;
};
export type Entry = {
  id: number;
  parentId: number | null;
  name: string;
  relativePath: string;
  kind: string;
  logicalBytes: number;
  allocatedBytes: number;
  modifiedAt: number | null;
  accessedAt: number | null;
  complete: boolean;
  error: string | null;
  category: string;
  disposition: string;
  reason: string;
  consequence: string;
  uncertainty: string;
  project: string | null;
  evidence: string[];
  removable: boolean;
};
export type EntryPage = {
  items: Entry[];
  total: number;
  totalBytes: number;
  offset: number;
};
export type DuplicateGroup = {
  hash: string;
  bytesEach: number;
  items: Entry[];
};
export type CleanupPlan = {
  token: string;
  scanId: number;
  mode: 'trash' | 'permanent';
  items: {
    id: number;
    relativePath: string;
    logicalBytes: number;
    reason: string;
    consequence: string;
  }[];
  estimatedBytes: number;
  expiresAt: number;
};
export type CleanupResult = {
  items: {
    id: number;
    relativePath: string;
    status: string;
    message: string;
    bytes: number;
  }[];
  movedBytes: number;
  deletedBytes: number;
  freeSpaceChange: number | null;
  interrupted: boolean;
};
export type ProviderSettings = {
  mode: ProviderMode;
  model: string;
  endpoint: string;
  keyStatus: 'missing' | 'saved' | 'session';
  status: string;
};
export type ProviderReport = {
  status: string;
  message: string;
  cached: boolean;
  decisions: {
    id: number;
    disposition: string;
    probability: number | null;
    confidence: number | null;
    refused: boolean;
  }[];
};
