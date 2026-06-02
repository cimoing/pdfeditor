<script setup lang="ts">
import { computed } from "vue";
import type { RichTextRun, TextTypography } from "../pdfEditor";

export interface FontOption {
  resource_name: string;
  family_name: string;
}

const props = defineProps<{
  runs: RichTextRun[];
  fontAssets: FontOption[];
  baseFontName: string | null;
  baseFontSize: number;
  baseColor: { r: number; g: number; b: number; a: number };
  typography: TextTypography;
  disabled?: boolean;
}>();

const emit = defineEmits<{
  "update:runs": [runs: RichTextRun[]];
  "update:typography": [typography: TextTypography];
  interact: [];
}>();

function newRunId() {
  return Math.random().toString(36).slice(2);
}

function updateRun(id: string, patch: Partial<RichTextRun>) {
  emit(
    "update:runs",
    props.runs.map((run) => (run.id === id ? { ...run, ...patch } : run))
  );
}

function removeRun(id: string) {
  const next = props.runs.filter((r) => r.id !== id);
  emit("update:runs", next.length ? next : [{ id: newRunId(), content: "", font_name: null, font_size: null, color: null }]);
}

function addRun() {
  const last = props.runs[props.runs.length - 1];
  emit("update:runs", [
    ...props.runs,
    { id: newRunId(), content: "", font_name: last?.font_name ?? null, font_size: last?.font_size ?? null, color: null }
  ]);
}

function moveRun(id: string, delta: -1 | 1) {
  const idx = props.runs.findIndex((r) => r.id === id);
  if (idx < 0) return;
  const next = [...props.runs];
  const swapIdx = idx + delta;
  if (swapIdx < 0 || swapIdx >= next.length) return;
  [next[idx], next[swapIdx]] = [next[swapIdx], next[idx]];
  emit("update:runs", next);
}

function updateTypography(patch: Partial<TextTypography>) {
  emit("update:typography", { ...props.typography, ...patch });
}

function beginPanelInteraction() {
  emit("interact");
}

function colorToHex(c: { r: number; g: number; b: number; a: number }) {
  return `#${[c.r, c.g, c.b].map((v) => v.toString(16).padStart(2, "0")).join("")}`;
}

function hexToColor(hex: string): { r: number; g: number; b: number; a: number } {
  const m = hex.match(/^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i);
  if (!m) return { r: 0, g: 0, b: 0, a: 255 };
  return { r: parseInt(m[1], 16), g: parseInt(m[2], 16), b: parseInt(m[3], 16), a: 255 };
}

function effectiveColor(run: RichTextRun) {
  return run.color ?? props.baseColor;
}

// All available fonts: page embedded fonts + "inherit" option
const fontOptions = computed(() => [
  { resource_name: "", family_name: "（继承）" },
  ...props.fontAssets
]);
</script>

<template>
  <div class="runs-editor" @pointerdown="beginPanelInteraction" @focusin="beginPanelInteraction">
    <div
      v-for="(run, idx) in runs"
      :key="run.id"
      class="run-row"
    >
      <div class="run-header">
        <span class="run-label">段 {{ idx + 1 }}</span>
        <div class="run-actions">
          <button :disabled="disabled || idx === 0" title="上移" @click="moveRun(run.id, -1)">↑</button>
          <button :disabled="disabled || idx === runs.length - 1" title="下移" @click="moveRun(run.id, 1)">↓</button>
          <button :disabled="disabled || runs.length <= 1" title="删除" class="run-delete" @click="removeRun(run.id)">×</button>
        </div>
      </div>

      <div class="run-content-row">
        <input
          class="run-content-input"
          :value="run.content"
          :disabled="disabled"
          placeholder="文本内容"
          spellcheck="false"
          @input="updateRun(run.id, { content: ($event.target as HTMLInputElement).value })"
        />
      </div>

      <div class="run-style-row">
        <label class="run-style-field">
          <span>字体</span>
          <select
            :value="run.font_name ?? ''"
            :disabled="disabled"
            @change="updateRun(run.id, { font_name: ($event.target as HTMLSelectElement).value || null })"
          >
            <option
              v-for="font in fontOptions"
              :key="font.resource_name"
              :value="font.resource_name"
            >{{ font.family_name }}</option>
          </select>
        </label>

        <label class="run-style-field run-size-field">
          <span>字号</span>
          <input
            type="number"
            :value="run.font_size ?? baseFontSize"
            :disabled="disabled"
            min="1"
            max="500"
            step="0.5"
            @change="updateRun(run.id, { font_size: parseFloat(($event.target as HTMLInputElement).value) || null })"
          />
        </label>

        <label class="run-style-field run-color-field">
          <span>颜色</span>
          <input
            type="color"
            :value="colorToHex(effectiveColor(run))"
            :disabled="disabled"
            @input="updateRun(run.id, { color: hexToColor(($event.target as HTMLInputElement).value) })"
          />
        </label>
      </div>
    </div>

    <button class="add-run-btn" :disabled="disabled" @click="addRun">+ 添加文字段</button>

    <div class="typography-controls">
      <label class="typography-toggle">
        <input
          type="checkbox"
          :checked="typography.replace_spaces_with_displacements"
          :disabled="disabled"
          @change="updateTypography({ replace_spaces_with_displacements: ($event.target as HTMLInputElement).checked })"
        />
        <span>空格写为 TJ 位移</span>
      </label>
      <label class="typography-toggle">
        <input
          type="checkbox"
          :checked="typography.compress_multi_punctuation"
          :disabled="disabled"
          @change="updateTypography({ compress_multi_punctuation: ($event.target as HTMLInputElement).checked })"
        />
        <span>多标点压缩识别</span>
      </label>
      <label class="typography-field">
        <span>数字字体</span>
        <select
          :value="typography.digit_font_name ?? ''"
          :disabled="disabled"
          @change="updateTypography({ digit_font_name: ($event.target as HTMLSelectElement).value || null })"
        >
          <option value="">（继承）</option>
          <option
            v-for="font in props.fontAssets"
            :key="`digit-${font.resource_name}`"
            :value="font.resource_name"
          >{{ font.family_name }}</option>
        </select>
      </label>
      <div class="typography-detected" v-if="typography.detected_tj_displacements || typography.detected_space_displacements || typography.detected_multi_punctuation || typography.detected_digit_font_name">
        识别：{{ [
          typography.detected_tj_displacements ? "TJ 位移" : "",
          typography.detected_space_displacements ? "位移空格" : "",
          typography.detected_multi_punctuation ? "多标点" : "",
          typography.detected_digit_font_name ? "数字字体" : ""
        ].filter(Boolean).join(" / ") }}
      </div>
    </div>
  </div>
</template>

<style scoped>
.runs-editor {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.run-row {
  border: 1px solid #d6dce2;
  border-radius: 4px;
  padding: 8px;
  background: #fff;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.run-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.run-label {
  font-size: 11px;
  color: #667085;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.run-actions {
  display: flex;
  gap: 4px;
}

.run-actions button {
  border: 1px solid #b9c3cd;
  background: #f5f7f9;
  cursor: pointer;
  padding: 1px 5px;
  font-size: 11px;
  border-radius: 3px;
  line-height: 1.4;
}

.run-actions button:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.run-delete {
  color: #b42318;
}

.run-content-input {
  width: 100%;
  border: 1px solid #b9c3cd;
  padding: 5px 7px;
  font-size: 13px;
  background: #fafbfc;
}

.run-style-row {
  display: flex;
  gap: 6px;
  align-items: flex-end;
}

.run-style-field {
  display: flex;
  flex-direction: column;
  gap: 3px;
  font-size: 11px;
  color: #475467;
  flex: 1;
  min-width: 0;
}

.run-style-field select,
.run-style-field input[type="number"] {
  border: 1px solid #b9c3cd;
  padding: 4px 5px;
  font-size: 12px;
  width: 100%;
  background: #fff;
}

.run-size-field {
  flex: 0 0 68px;
}

.run-color-field {
  flex: 0 0 44px;
}

.run-color-field input[type="color"] {
  width: 36px;
  height: 28px;
  padding: 1px 2px;
  border: 1px solid #b9c3cd;
  cursor: pointer;
}

.add-run-btn {
  width: 100%;
  padding: 7px;
  border: 1px dashed #b9c3cd;
  background: transparent;
  cursor: pointer;
  font-size: 12px;
  color: #475467;
  border-radius: 4px;
  transition: background 0.15s;
}

.add-run-btn:hover:not(:disabled) {
  background: #f5f7f9;
}

.add-run-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.typography-controls {
  display: grid;
  gap: 8px;
  padding-top: 8px;
  border-top: 1px solid #e4e7ec;
  font-size: 12px;
}

.typography-toggle {
  display: flex;
  align-items: center;
  gap: 8px;
  color: #344054;
}

.typography-field {
  display: grid;
  gap: 4px;
  color: #344054;
}

.typography-field select {
  width: 100%;
  border: 1px solid #b9c3cd;
  background: #fff;
  padding: 6px 8px;
  font-size: 12px;
}

.typography-detected {
  color: #667085;
}
</style>
