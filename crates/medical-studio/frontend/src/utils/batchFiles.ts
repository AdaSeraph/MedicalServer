export interface BatchImportItem {
  name: string;
  content_text?: string;
  content_base64?: string;
  file_path?: string;
}

export interface QueuedImportFile {
  id: string;
  name: string;
  size: number;
  kind: 'file' | 'paste';
  item: BatchImportItem;
}

const BINARY_EXTS = new Set(['docx', 'pdf', 'xlsx', 'xls']);

function getExtension(name: string): string {
  const idx = name.lastIndexOf('.');
  return idx >= 0 ? name.slice(idx + 1).toLowerCase() : '';
}

function arrayBufferToBase64(buffer: ArrayBuffer): string {
  const bytes = new Uint8Array(buffer);
  const chunkSize = 0x8000;
  let binary = '';
  for (let i = 0; i < bytes.length; i += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunkSize));
  }
  return btoa(binary);
}

export async function readBrowserFilesToQueue(fileList: FileList | File[]): Promise<QueuedImportFile[]> {
  const files = Array.from(fileList);
  const out: QueuedImportFile[] = [];

  for (const file of files) {
    const ext = getExtension(file.name);
    const id = `${file.name}-${file.size}-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
    if (BINARY_EXTS.has(ext)) {
      const buf = await file.arrayBuffer();
      out.push({
        id,
        name: file.name,
        size: file.size,
        kind: 'file',
        item: {
          name: file.name,
          content_base64: arrayBufferToBase64(buf),
        },
      });
    } else {
      const text = await file.text();
      out.push({
        id,
        name: file.name,
        size: file.size,
        kind: 'file',
        item: {
          name: file.name,
          content_text: text,
        },
      });
    }
  }

  return out;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
}
