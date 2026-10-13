/** Rust-owned paths and next-start setting; never reconstruct model roots in the UI. */
export type ModelStorageStatus = {
  defaultParent: string;
  dataDir: string;
  databasePath: string;
  activeParent: string;
  models: string;
  embedModels: string;
  customModels: string;
  savedParent: string | null;
  pendingRestart: boolean;
  error: string | null;
};
