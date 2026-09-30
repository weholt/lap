
import { defineStore } from 'pinia';

export const useUIStore = defineStore('ui', {
  state: () => ({
    activePane: 'content',
    inputStack: [],
    fileVersions: {},
    // A count is written only in response to an explicit sidebar item activation.
    // Content consumes this after its matching query has completed.
    countUpdateRequest: null,
    countUpdateTick: 0,
    mapActive: false,
    activeAdjustments: {
      filePath: null,
      brightness: 0,
      contrast: 0,
      saturation: 100,
      hue: 0,
      blur: 0,
      filter: null,
      resize: null
    },
    // Native develop editing state (lap-0e9 / TASK-303). Runtime timers and
    // the backend session stay in the useDevelopEditor composable; this state
    // is the shared, serializable projection other components query.
    developEditor: {
      activeAssetId: null,
      dirty: false,
      saveState: 'idle', // idle | pending | saving | saved | conflict | failed
      lastError: null,
      // assetId -> { recipe, saveState, lastError } for assets left with
      // uncommitted edits (failed save or pending debounce) so their state is
      // retained for retry and never leaks into another asset.
      retained: {}
    }
  }),
  getters: {
    isInputActive: (state) => (name) => {
      return state.inputStack.length > 0 && state.inputStack[state.inputStack.length - 1] === name;
    },
    hasDirtyDevelopState: (state) => (assetId) => {
      const id = Number(assetId);
      if (!id || !Number.isFinite(id)) return false;
      const dev = state.developEditor;
      if (dev.activeAssetId === id && (dev.dirty || dev.saveState === 'pending')) return true;
      return Boolean(dev.retained[id]);
    },
    getFileVersion: (state) => (filePath) => {
      return state.fileVersions[filePath] || 0;
    },
    hasActiveChanges: (state) => (fileInfo) => {
      if (!state.activeAdjustments.filePath || !fileInfo) return false;
      if (state.activeAdjustments.filePath !== fileInfo.file_path) return false;

      const adj = state.activeAdjustments;
      const hasAdjustments = adj.brightness !== 0 || 
                             adj.contrast !== 0 || 
                             adj.saturation !== 100 || 
                             adj.hue !== 0 || 
                             adj.blur !== 0 || 
                             !!adj.filter;

      let hasResize = false;
      if (adj.resize) {
        hasResize = Math.round(adj.resize.width) !== Math.round(fileInfo.width) || 
                    Math.round(adj.resize.height) !== Math.round(fileInfo.height);
      }

      return hasAdjustments || hasResize;
    }
  },
  actions: {
    requestCountUpdate(request) {
      this.countUpdateRequest = request;
      this.countUpdateTick++;
    },
    clearCountUpdateRequest() {
      this.countUpdateRequest = null;
    },
    setActivePane(pane) {
      this.activePane = pane;
    },
    pushInputHandler(name) {
      this.inputStack.push(name);
    },
    popInputHandler() {
      this.inputStack.pop();
    },
    removeInputHandler(name) {
      const index = this.inputStack.indexOf(name);
      if (index !== -1) {
        this.inputStack.splice(index, 1);
      }
    },
    updateFileVersion(filePath) {
      const currentVersion = this.fileVersions[filePath] || 0;
      this.fileVersions[filePath] = currentVersion + 1;
    },
    setMapActive(active) {
      this.mapActive = !!active;
    },
    setActiveAdjustments(filePath, adjustments) {
      this.activeAdjustments = {
        ...this.activeAdjustments,
        filePath,
        ...adjustments
      };
    },
    clearActiveAdjustments() {
      this.activeAdjustments = {
        filePath: null,
        brightness: 0,
        contrast: 0,
        saturation: 100,
        hue: 0,
        blur: 0,
        filter: null,
        resize: null
      };
    },
    setDevelopActive(assetId) {
      this.developEditor.activeAssetId = assetId === null ? null : Number(assetId);
    },
    setDevelopDirty(dirty) {
      this.developEditor.dirty = !!dirty;
    },
    /** @param {string} saveState @param {string | null} [lastError] */
    setDevelopSaveState(saveState, lastError = null) {
      this.developEditor.saveState = saveState;
      this.developEditor.lastError = lastError;
    },
    retainDevelopState(assetId, entry) {
      const id = Number(assetId);
      if (!id || !Number.isFinite(id)) return;
      this.developEditor.retained[id] = entry;
    },
    takeRetainedDevelopState(assetId) {
      const id = Number(assetId);
      if (!id || !Number.isFinite(id)) return null;
      const entry = this.developEditor.retained[id] || null;
      if (entry) {
        delete this.developEditor.retained[id];
      }
      return entry;
    },
    peekRetainedDevelopState(assetId) {
      const id = Number(assetId);
      if (!id || !Number.isFinite(id)) return null;
      return this.developEditor.retained[id] || null;
    },
    clearRetainedDevelopState(assetId) {
      const id = Number(assetId);
      if (!id || !Number.isFinite(id)) return;
      delete this.developEditor.retained[id];
    }
  },
});
