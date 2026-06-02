import { ref } from "vue";
import type { TextEditSessionInfo, TextLayoutPreview, RichTextRun } from "../pdfEditor";

export function usePdfEditor() {
  const selectedTextId = ref<number | null>(null);
  const editSession = ref<TextEditSessionInfo | null>(null);
  const draftText = ref("");
  const draftRuns = ref<RichTextRun[]>([]);
  const layoutPreview = ref<TextLayoutPreview | null>(null);
  const isPreparingEdit = ref(false);
  const isSavingEdit = ref(false);

  let selectionSequence = 0;
  let previewRequestSequence = 0;
  let previewTimer: number | null = null;

  function clearPreviewTimer() {
    if (previewTimer != null) {
      window.clearTimeout(previewTimer);
      previewTimer = null;
    }
  }

  function clearEditingState() {
    clearPreviewTimer();
    selectionSequence += 1;
    previewRequestSequence += 1;
    selectedTextId.value = null;
    editSession.value = null;
    draftText.value = "";
    draftRuns.value = [];
    layoutPreview.value = null;
    isPreparingEdit.value = false;
    isSavingEdit.value = false;
  }

  function getSelectionToken() {
    return selectionSequence;
  }

  function incrementSelection() {
    return ++selectionSequence;
  }

  function getPreviewToken() {
    return previewRequestSequence;
  }

  function incrementPreviewToken() {
    return ++previewRequestSequence;
  }

  function setPreviewTimer(callback: () => void, delayMs: number) {
    clearPreviewTimer();
    previewTimer = window.setTimeout(callback, delayMs);
  }

  return {
    selectedTextId,
    editSession,
    draftText,
    draftRuns,
    layoutPreview,
    isPreparingEdit,
    isSavingEdit,
    clearPreviewTimer,
    clearEditingState,
    getSelectionToken,
    incrementSelection,
    getPreviewToken,
    incrementPreviewToken,
    setPreviewTimer
  };
}
