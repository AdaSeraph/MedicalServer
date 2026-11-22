import { ref } from 'vue';

export type StudioTheme = 'dark' | 'light';

const STORAGE_KEY = 'medical-studio-theme';
const theme = ref<StudioTheme>('dark');

function applyTheme(next: StudioTheme) {
  theme.value = next;
  if (typeof document !== 'undefined') {
    document.documentElement.classList.toggle('theme-light', next === 'light');
  }
  try {
    localStorage.setItem(STORAGE_KEY, next);
  } catch {
    // ignore storage quota errors
  }
}

export function initTheme() {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved === 'light' || saved === 'dark') {
      applyTheme(saved);
      return;
    }
  } catch {
    // ignore
  }
  applyTheme('dark');
}

export function useTheme() {
  function toggleTheme() {
    applyTheme(theme.value === 'dark' ? 'light' : 'dark');
  }

  return {
    theme,
    setTheme: applyTheme,
    toggleTheme,
  };
}
