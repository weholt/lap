import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount } from "@vue/test-utils";
import { createI18n } from "vue-i18n";
import { createPinia, setActivePinia } from "pinia";

// ImageEditor regressions for developed-asset geometry (lap-6bc / TASK-402):
// the editor window hosts the shared GeometryControls for developed assets,
// and a derivative export always renders the SAME transforms the user sees,
// because pending geometry edits are committed (flushed) before the export
// resolves its immutable recipe revision.

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

const closeWindow = vi.fn(async () => {});
const destroyWindow = vi.fn(async () => {});

vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({
    close: closeWindow,
    destroy: destroyWindow,
    isVisible: vi.fn(async () => false),
    listen: vi.fn(async () => () => {}),
  }),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    isMaximized: vi.fn(async () => false),
    maximize: vi.fn(async () => {}),
    unmaximize: vi.fn(async () => {}),
    minimize: vi.fn(async () => {}),
    close: closeWindow,
    destroy: destroyWindow,
    onResized: vi.fn(async () => () => {}),
    listen: vi.fn(async () => () => {}),
  }),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}));

const routerQuery: Record<string, string> = { fileId: "7" };

vi.mock("vue-router", () => ({
  useRouter: () => ({
    currentRoute: { value: { query: routerQuery } },
  }),
}));

import ImageEditor from "@/views/ImageEditor.vue";
import { useDevelopEditor } from "@/composables/useDevelopEditor";
import {
  DEFAULT_RECIPE,
  RECIPE_SCHEMA_VERSION,
  type ExportReceipt,
} from "@/composables/useDevelopSession.types";
import enMessages from "@/locales/en.json";

const commandResponses = new Map<string, unknown[]>();

function queueCommand(command: string, response: unknown) {
  if (!commandResponses.has(command)) commandResponses.set(command, []);
  commandResponses.get(command)!.push(response);
}

function invokeCalls(command: string): any[] {
  return invokeMock.mock.calls.filter(([name]) => name === command);
}

function developedFile() {
  return {
    id: 7,
    name: "photo.CR2",
    file_path: "C:/photos/photo.CR2",
    file_type: 3,
    width: 3000,
    height: 2000,
    modified_at: 1,
    e_orientation: 1,
  };
}

function openedSession(revision = 0) {
  return {
    sessionId: 107,
    assetId: "7",
    variantId: "default",
    revision,
    dimensions: [3000, 2000],
    sourceFingerprint: "f".repeat(64),
    envelope: {
      schemaVersion: RECIPE_SCHEMA_VERSION,
      engineVersion: "lap/0.3.2/rapidraw-edit-model/0.1.0",
      assetId: "7",
      variantId: "default",
      revision,
      sourceFingerprint: "f".repeat(64),
      decode: {},
      recipe: structuredClone(DEFAULT_RECIPE),
      resources: {},
      unsupported: {},
    },
  };
}

function completedTicket(generation = 1) {
  return {
    status: "completed",
    ticket: {
      sessionId: 107,
      assetId: "7",
      variantId: "default",
      generation,
      quality: "settled",
      width: 8,
      height: 8,
      handle: `handle-${generation}`,
      byteLen: 8 * 8 * 4,
    },
  };
}

function exportReceipt(): ExportReceipt {
  return {
    assetId: "7",
    variantId: "default",
    revision: 1,
    destination: "C:/photos/photo_1.jpg",
    format: "jpeg",
    width: 1500,
    height: 1000,
    bytesWritten: 10,
    sourceFingerprint: "f".repeat(64),
    contentHash: "c".repeat(64),
  };
}

function makeI18n() {
  return createI18n({
    legacy: false,
    locale: "en",
    fallbackLocale: "en",
    messages: { en: enMessages },
  });
}

async function mountEditor() {
  const wrapper = mount(ImageEditor, {
    global: {
      plugins: [makeI18n(), setActivePinia(createPinia())],
    },
  });
  await flushPromises();
  return wrapper;
}

describe("ImageEditor developed-asset geometry", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockImplementation(async (command: string) => {
      const queue = commandResponses.get(command);
      if (!queue) throw new Error(`unexpected invoke: ${command}`);
      const response = queue.shift();
      if (response instanceof Error) throw response;
      return response;
    });
    commandResponses.clear();
    routerQuery.fileId = "7";
    setActivePinia(createPinia());
    closeWindow.mockClear();
    destroyWindow.mockClear();
  });

  afterEach(async () => {
    commandResponses.clear();
    await useDevelopEditor().disposeForTests();
    vi.restoreAllMocks();
  });

  function queueDevelopedAssetOpen() {
    queueCommand("get_file_info", developedFile());
    // check_file_exists: (1) developed sidecar probe, (2) save-as-new
    // candidate-name collision probe.
    queueCommand("check_file_exists", true);
    queueCommand("check_file_exists", false);
    queueCommand("develop_open_edit_session", openedSession());
    queueCommand("develop_render_preview", completedTicket(1));
    queueCommand(
      "develop_take_preview_frame",
      new Uint8Array(8 * 8 * 4).buffer,
    );
  }

  it("hosts the shared geometry controls for a developed asset", async () => {
    queueDevelopedAssetOpen();
    const wrapper = await mountEditor();
    await flushPromises();

    expect(wrapper.find('[data-testid="develop-geometry-root"]').exists()).toBe(
      true,
    );
    // The legacy CSS crop overlay never activates for developed assets.
    expect(wrapper.find(".crop-box-active").exists()).toBe(false);
  });

  it("flushes pending geometry edits before exporting the derivative", async () => {
    vi.useFakeTimers();
    try {
      queueDevelopedAssetOpen();
      const wrapper = await mountEditor();
      const editor = useDevelopEditor();
      expect(editor.recipe.value).not.toBeNull();

      // Pending geometry edit: crop + one quarter turn.
      editor.applyRecipePatch(
        {
          orientationSteps: 1,
          crop: { x: 0.1, y: 0.1, width: 0.5, height: 0.5 },
        },
        "rotate",
      );
      queueCommand("develop_commit_recipe", {
        sessionId: 107,
        revision: 1,
        contentHash: "c".repeat(64),
        sidecarPath: "C:/photos/photo.CR2.lapedit.json",
        projectionApplied: true,
        projectionError: null,
      });
      queueCommand("develop_export_developed", {
        status: "completed",
        receipt: exportReceipt(),
      });

      const saveButton = wrapper
        .findAll("button")
        .find((b) => b.classes().join(" ").includes("btn-primary"));
      expect(saveButton, "primary save button").toBeTruthy();
      await saveButton!.trigger("click");
      await vi.advanceTimersByTimeAsync(0);
      await flushPromises();

      const commitIndex = invokeMock.mock.calls.findIndex(
        ([name]) => name === "develop_commit_recipe",
      );
      const exportIndex = invokeMock.mock.calls.findIndex(
        ([name]) => name === "develop_export_developed",
      );
      expect(commitIndex).toBeGreaterThanOrEqual(0);
      expect(exportIndex).toBeGreaterThan(commitIndex);

      // The export rendered exactly the committed revision and the
      // committed recipe carries the pending geometry.
      const commitEnvelope = invokeCalls("develop_commit_recipe")[0][1];
      expect(commitEnvelope.expectedRevision).toBe(0);
      expect(commitEnvelope.envelope.recipe.orientationSteps).toBe(1);
      expect(commitEnvelope.envelope.recipe.crop).toEqual({
        x: 0.1,
        y: 0.1,
        width: 0.5,
        height: 0.5,
      });
      const exportArgs = invokeCalls("develop_export_developed")[0][1];
      expect(exportArgs.revision).toBe(1);
      expect(exportArgs.destination).not.toBe("C:/photos/photo.CR2");
    } finally {
      vi.useRealTimers();
    }
  });
});
