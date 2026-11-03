<script setup lang="ts">
defineProps<{
  path: string;
  src: string;
  name: string;
}>();
</script>

<template>
  <div class="img-preview">
    <div class="img-preview-meta muted">
      <span :title="path">{{ name }}</span>
      <span>只读预览</span>
    </div>
    <div class="img-preview-stage">
      <img :src="src" :alt="name" class="img-preview-el" />
    </div>
  </div>
</template>

<style scoped>
.img-preview {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  background: var(--panel, #f7f9fc);
}
.img-preview-meta {
  display: flex;
  justify-content: space-between;
  gap: 0.75rem;
  padding: 0.35rem 0.75rem;
  border-bottom: 1px solid var(--border, #c5ced9);
  font-size: 0.85em;
}
.img-preview-stage {
  flex: 1;
  min-height: 0;
  display: grid;
  place-items: center;
  overflow: auto;
  padding: 1rem;
  background-color: color-mix(in srgb, var(--panel, #f7f9fc) 70%, #888);
  background-image:
    linear-gradient(45deg, rgba(0, 0, 0, 0.06) 25%, transparent 25%),
    linear-gradient(-45deg, rgba(0, 0, 0, 0.06) 25%, transparent 25%),
    linear-gradient(45deg, transparent 75%, rgba(0, 0, 0, 0.06) 75%),
    linear-gradient(-45deg, transparent 75%, rgba(0, 0, 0, 0.06) 75%);
  background-size: 16px 16px;
  background-position:
    0 0,
    0 8px,
    8px -8px,
    -8px 0;
}
.img-preview-el {
  max-width: 100%;
  max-height: 100%;
  object-fit: contain;
  box-shadow: 0 4px 24px rgba(0, 0, 0, 0.18);
  background: transparent;
}
</style>
