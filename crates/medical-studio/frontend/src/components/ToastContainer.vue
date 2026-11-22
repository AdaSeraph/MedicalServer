<script setup lang="ts">
import { useToast } from '../composables/useToast';

const { toasts, dismiss } = useToast();
</script>

<template>
  <div class="fixed top-4 right-4 z-50 flex flex-col gap-2 w-80 max-w-[calc(100vw-2rem)] pointer-events-none">
    <TransitionGroup name="toast">
      <div
        v-for="t in toasts"
        :key="t.id"
        :class="{
          'bg-emerald-950/95 border-emerald-600/80 text-emerald-100': t.kind === 'success',
          'bg-rose-950/95 border-rose-600/80 text-rose-100': t.kind === 'error',
          'bg-amber-950/95 border-amber-600/80 text-amber-100': t.kind === 'warning',
          'bg-slate-900/95 border-sky-600/70 text-sky-100': t.kind === 'info',
        }"
        class="pointer-events-auto px-3.5 py-2.5 rounded-xl border shadow-xl backdrop-blur-md text-xs leading-relaxed flex items-start justify-between gap-2.5"
      >
        <span class="flex-1 break-words">{{ t.message }}</span>
        <button
          @click="dismiss(t.id)"
          class="opacity-70 hover:opacity-100 shrink-0 px-1 text-[11px]"
          aria-label="关闭通知"
        >
          ✕
        </button>
      </div>
    </TransitionGroup>
  </div>
</template>

<style scoped>
.toast-enter-active,
.toast-leave-active {
  transition: all 0.22s ease;
}
.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translateX(20px);
}
</style>
