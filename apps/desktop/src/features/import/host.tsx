import { create } from 'zustand';
import type { ExportFormat } from '@/lib/api/import';
import { ExportDialog } from './ExportDialog';
import { ImportDialog } from './ImportDialog';

interface ImportExportState {
  importFolder: string | null;
  exportTarget: { folder: string; format?: ExportFormat } | null;
  openImport(folder?: string): void;
  openExport(folder?: string, format?: ExportFormat): void;
  close(): void;
}

/** Opens the import/export dialogs from anywhere (tree menus, empty states, command palette). */
export const useImportExport = create<ImportExportState>((set) => ({
  importFolder: null,
  exportTarget: null,
  openImport: (folder = '') => set({ importFolder: folder, exportTarget: null }),
  openExport: (folder = '', format) => set({ exportTarget: { folder, format }, importFolder: null }),
  close: () => set({ importFolder: null, exportTarget: null }),
}));

/** Mounted once in App. */
export function ImportExportHost() {
  const { importFolder, exportTarget, close } = useImportExport();
  return (
    <>
      {importFolder !== null && <ImportDialog open targetFolder={importFolder} onClose={close} />}
      {exportTarget && <ExportDialog open folder={exportTarget.folder} format={exportTarget.format} onClose={close} />}
    </>
  );
}
