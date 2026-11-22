import { ref } from 'vue';

export type ToastKind = 'info' | 'success' | 'error' | 'warning';

export interface ToastItem {
  id: number;
  message: string;
  kind: ToastKind;
}

const toasts = ref<ToastItem[]>([]);
let nextId = 1;

export function useToast() {
  function push(message: string, kind: ToastKind = 'info', durationMs = 3600) {
    const id = nextId++;
    toasts.value.push({ id, message, kind });
    if (durationMs > 0) {
      window.setTimeout(() => {
        dismiss(id);
      }, durationMs);
    }
  }

  function dismiss(id: number) {
    toasts.value = toasts.value.filter((t) => t.id !== id);
  }

  return {
    toasts,
    push,
    dismiss,
    info: (m: string) => push(m, 'info'),
    success: (m: string) => push(m, 'success'),
    error: (m: string) => push(m, 'error', 5200),
    warning: (m: string) => push(m, 'warning', 4500),
  };
}
