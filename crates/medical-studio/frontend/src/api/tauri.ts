import { invoke } from '@tauri-apps/api/core';

export async function invokeStudio<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return invoke<T>(command, args);
}
