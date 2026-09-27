import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount } from "@vue/test-utils";
import { createI18n } from "vue-i18n";
import { createPinia, setActivePinia } from "pinia";

// Component regressions for the develop geometry controls (lap-6bc /
// TASK-402; managed continuation of lap-002.2). Spec refs A5/A6/A9/A10:
// crop/rotate/flip interactions in oriented coordinates with handle drags,
// aspect locks, keyboard control, transaction commit/cancel, undo/reset
// participation, and empty/out-of-range crop rejection.

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import GeometryControls from "@/components/develop/GeometryControls.vue";
import { useDevelopEditor } from "@/composables/useDevelopEditor";
import {
  cropToSourceRect,
  GEOMETRY_FRAME_MAX_EDGE,
  orientedDimensions,
} from "@/composables/useDevelopGeometry";
import {
  DEFAULT_RECIPE,
  RECIPE_SCHEMA_VERSION,
  type CropRect,
} from "@/composables/useDevelopSession.types";
import enMessages from "@/locales/en.json";
import deMessages from "@/locales/de.json";
import frMessages from "@/locales/fr.json";
import jaMessages from "@/locales/ja.json";
import zhMessages from "@/locales/zh.json";
import esMessages from "@/locales/es.json";
import koMessages from "@/locales/ko.json";
import ptMessages from "@/locales/pt.json";
import ruMessages from "@/locales/ru.json";

const LOCALE_MESSAGES: Record<string, Record<string, unknown>> = {
  en: enMessages as Record<string, unknown>,
  de: deMessages as Record<string, unknown>,
  fr: frMessages as Record<string, unknown>,
  ja: jaMessages as Record<string, unknown>,
  zh: zhMessages as Record<string, unknown>,
  es: esMessages as Record<string, unknown>,
  ko: koMessages as Record<string, unknown>,
  pt: ptMessages as Record<string, unknown>,
  ru: ruMessages as Record<string, unknown>,
};

function makeI18n(locale = "en") {
  return createI18n({
    legacy: false,
    locale,
    fallbackLocale: "en",
    messages: LOCALE_MESSAGES,
  });
}

const commandResponses = new Map<string, unknown[]>();

function queueCommand(command: string, response: unknown) {
  if (!commandResponses.has(command)) commandResponses.set(command, []);
  commandResponses.get(command)!.push(response);
}

function openedSession(assetId: number, revision = 0) {
  return {
    sessionId: 100 + assetId,
    assetId: String(assetId),
    variantId: "default",
    revision,
    dimensions: [3000, 2000],
    sourceFingerprint: "f".repeat(64),
    envelope: {
      schemaVersion: RECIPE_SCHEMA_VERSION,
      engineVersion: "lap/0.3.2/rapidraw-edit-model/0.1.0",
      assetId: String(assetId),
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

function completedTicket(assetId: number) {
  return {
    status: "completed",
    ticket: {
      sessionId: 100 + assetId,
      assetId: String(assetId),
      variantId: "default",
      generation: 1,
      quality: "settled",
      width: 8,
      height: 8,
      handle: `handle-${assetId}`,
      byteLen: 8 * 8 * 4,
    },
  };
}

async function mountControls(locale = "en") {
  const editor = useDevelopEditor();
  queueCommand("develop_open_edit_session", openedSession(7));
  queueCommand("develop_render_preview", completedTicket(7));
  queueCommand("develop_take_preview_frame", new Uint8Array(8 * 8 * 4).buffer);
  await editor.openAsset({ id: 7 });
  const wrapper = mount(GeometryControls, {
    global: {
      plugins: [makeI18n(locale), setActivePinia(createPinia())],
    },
  });
  await flushPromises();
  return wrapper;
}

const IDENTITY = { steps: 0, flipHorizontal: false, flipVertical: false };

/** The rendered crop-editor frame width in px for the 3000x2000 fixture. */
function frameWidthFor(sourceW: number, sourceH: number): number {
  const { width, height } = orientedDimensions(sourceW, sourceH, 0);
  return width >= height
    ? GEOMETRY_FRAME_MAX_EDGE
    : Math.max(1, Math.round(GEOMETRY_FRAME_MAX_EDGE * (width / height)));
}

function windowPointer(type: string, clientX: number, clientY: number) {
  window.dispatchEvent(new MouseEvent(type, { clientX, clientY }));
}

describe("GeometryControls", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockImplementation(async (command: string) => {
      const queue = commandResponses.get(command);
      if (!queue) throw new Error(`unexpected develop invoke: ${command}`);
      const response = queue.shift();
      if (response instanceof Error) throw response;
      return response;
    });
    commandResponses.clear();
    setActivePinia(createPinia());
  });

  afterEach(async () => {
    commandResponses.clear();
    await useDevelopEditor().disposeForTests();
    vi.restoreAllMocks();
  });

  it("rotates right with one undo step and keeps the crop on the same source pixels", async () => {
    const wrapper = await mountControls();
    const editor = useDevelopEditor();
    editor.applyRecipePatch(
      { crop: { x: 0.1, y: 0.2, width: 0.5, height: 0.3 } },
      "seed crop",
    );
    const seeded = editor.historySize.value;

    await wrapper
      .get('[data-testid="develop-geometry-rotate-right"]')
      .trigger("click");

    expect(editor.recipe.value?.orientationSteps).toBe(1);
    const crop = editor.recipe.value?.crop;
    expect(crop).not.toBeNull();
    // The crop follows the image: identical decoded pixels stay selected.
    expect(
      cropToSourceRect(crop, 3000, 2000, {
        steps: 1,
        flipHorizontal: false,
        flipVertical: false,
      }),
    ).toEqual(
      cropToSourceRect(
        { x: 0.1, y: 0.2, width: 0.5, height: 0.3 },
        3000,
        2000,
        IDENTITY,
      ),
    );
    expect(editor.historySize.value).toBe(seeded + 1);

    editor.undo();
    expect(editor.recipe.value?.orientationSteps).toBe(0);
    expect(wrapper.find('[data-testid="develop-geometry-root"]').exists()).toBe(
      true,
    );
  });

  it("rotates left across the wrap-around and mirrors with the flip controls", async () => {
    const wrapper = await mountControls();
    const editor = useDevelopEditor();

    await wrapper
      .get('[data-testid="develop-geometry-rotate-left"]')
      .trigger("click");
    expect(editor.recipe.value?.orientationSteps).toBe(3);

    await wrapper
      .get('[data-testid="develop-geometry-rotate-left"]')
      .trigger("click");
    expect(editor.recipe.value?.orientationSteps).toBe(2);

    await wrapper
      .get('[data-testid="develop-geometry-flip-horizontal"]')
      .trigger("click");
    expect(editor.recipe.value?.flipHorizontal).toBe(true);

    await wrapper
      .get('[data-testid="develop-geometry-flip-vertical"]')
      .trigger("click");
    expect(editor.recipe.value?.flipVertical).toBe(true);

    editor.undo();
    expect(editor.recipe.value?.flipVertical).toBe(false);
    expect(editor.recipe.value?.flipHorizontal).toBe(true);
  });

  it("applies an aspect lock to the current crop and clears it on free", async () => {
    const wrapper = await mountControls();
    const editor = useDevelopEditor();
    editor.applyRecipePatch(
      { crop: { x: 0.25, y: 0.25, width: 0.5, height: 0.5 } },
      "seed crop",
    );

    const select = wrapper.get('[data-testid="develop-geometry-aspect"]');
    await select.setValue("1.5");

    expect(editor.recipe.value?.aspectRatio).toBe(1.5);
    const crop = editor.recipe.value?.crop;
    expect(crop).not.toBeNull();
    // The aspect is a pixel ratio over the oriented (3000x2000) frame.
    expect((crop!.width * 3000) / (crop!.height * 2000)).toBeCloseTo(1.5, 3);
    expect(crop!.x).toBeGreaterThanOrEqual(0);
    expect(crop!.x + crop!.width).toBeLessThanOrEqual(1);

    await select.setValue("free");
    expect(editor.recipe.value?.aspectRatio).toBeNull();
  });

  it("moves the crop with the keyboard, one transaction per keystroke", async () => {
    const wrapper = await mountControls();
    const editor = useDevelopEditor();
    editor.applyRecipePatch(
      { crop: { x: 0.2, y: 0.2, width: 0.4, height: 0.4 } },
      "seed",
    );
    const seeded = editor.historySize.value;

    const frame = wrapper.get('[data-testid="develop-geometry-crop"]');
    await frame.trigger("keydown", { key: "ArrowRight" });
    await frame.trigger("keydown", { key: "ArrowDown", shiftKey: true });

    const crop = editor.recipe.value?.crop!;
    expect(crop.x).toBeCloseTo(0.21, 6);
    expect(crop.y).toBeCloseTo(0.25, 6);
    expect(editor.historySize.value).toBe(seeded + 2);
  });

  it("drags a resize handle live and commits exactly one transaction on pointer up", async () => {
    const wrapper = await mountControls();
    const editor = useDevelopEditor();
    editor.applyRecipePatch(
      { crop: { x: 0.2, y: 0.2, width: 0.4, height: 0.4 } },
      "seed",
    );
    const seeded = editor.historySize.value;

    const frameW = frameWidthFor(3000, 2000);
    const handle = wrapper.get('[data-testid="develop-geometry-handle-se"]');
    await handle.trigger("pointerdown", {
      clientX: 100,
      clientY: 100,
      button: 0,
    });
    windowPointer("pointermove", 100 + 40, 100);
    windowPointer("pointerup", 100 + 40, 100);

    const crop = editor.recipe.value?.crop!;
    expect(crop.width).toBeCloseTo(0.4 + 40 / frameW, 6);
    expect(crop.x).toBeCloseTo(0.2, 6);
    expect(editor.historySize.value).toBe(seeded + 1);
  });

  it("drags the whole crop box and clamps it to the oriented frame", async () => {
    const wrapper = await mountControls();
    const editor = useDevelopEditor();
    editor.applyRecipePatch(
      { crop: { x: 0.1, y: 0.1, width: 0.3, height: 0.3 } },
      "seed",
    );

    const box = wrapper.get('[data-testid="develop-geometry-crop-box"]');
    await box.trigger("pointerdown", { clientX: 50, clientY: 50, button: 0 });
    // Way beyond the frame: the move must clamp, never leave the frame.
    windowPointer("pointermove", 10000, 10000);
    windowPointer("pointerup", 10000, 10000);

    const crop = editor.recipe.value?.crop!;
    expect(crop.x + crop.width).toBeCloseTo(1, 6);
    expect(crop.y + crop.height).toBeCloseTo(1, 6);
    expect(editor.recipe.value?.crop).not.toBeNull();
  });

  it("cancels an open drag gesture with Escape and restores the exact prior crop", async () => {
    const wrapper = await mountControls();
    const editor = useDevelopEditor();
    const seededCrop: CropRect = { x: 0.2, y: 0.2, width: 0.4, height: 0.4 };
    editor.applyRecipePatch({ crop: seededCrop }, "seed");
    const seeded = editor.historySize.value;

    const handle = wrapper.get('[data-testid="develop-geometry-handle-se"]');
    await handle.trigger("pointerdown", {
      clientX: 100,
      clientY: 100,
      button: 0,
    });
    windowPointer("pointermove", 160, 160);
    expect(editor.recipe.value?.crop!.width).toBeGreaterThan(0.4);

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(editor.recipe.value?.crop).toEqual(seededCrop);
    expect(editor.historySize.value).toBe(seeded);

    // The cancelled gesture left no open transaction behind.
    expect(editor.canUndo.value).toBe(true);
  });

  it("rejects degenerate crop drags instead of persisting an empty crop", async () => {
    const wrapper = await mountControls();
    const editor = useDevelopEditor();
    editor.applyRecipePatch(
      { crop: { x: 0.2, y: 0.2, width: 0.4, height: 0.4 } },
      "seed",
    );

    const handle = wrapper.get('[data-testid="develop-geometry-handle-se"]');
    await handle.trigger("pointerdown", {
      clientX: 100,
      clientY: 100,
      button: 0,
    });
    // Collapse the crop far past its top-left corner: clamped to the
    // minimum size, never an empty/invalid rectangle.
    windowPointer("pointermove", -5000, -5000);
    windowPointer("pointerup", -5000, -5000);

    const crop = editor.recipe.value?.crop!;
    expect(crop.width).toBeGreaterThan(0);
    expect(crop.height).toBeGreaterThan(0);
    expect(crop.x).toBeGreaterThanOrEqual(0);
    expect(crop.y).toBeGreaterThanOrEqual(0);
  });

  it("resets the geometry to defaults with one undoable transaction", async () => {
    const wrapper = await mountControls();
    const editor = useDevelopEditor();
    editor.applyRecipePatch(
      {
        orientationSteps: 3,
        flipHorizontal: true,
        flipVertical: true,
        crop: { x: 0.1, y: 0.1, width: 0.5, height: 0.5 },
        aspectRatio: 1,
      },
      "seed",
    );

    await wrapper
      .get('[data-testid="develop-geometry-reset"]')
      .trigger("click");

    expect(editor.recipe.value?.orientationSteps).toBe(0);
    expect(editor.recipe.value?.flipHorizontal).toBe(false);
    expect(editor.recipe.value?.flipVertical).toBe(false);
    expect(editor.recipe.value?.crop).toBeNull();
    expect(editor.recipe.value?.aspectRatio).toBeNull();

    editor.undo();
    expect(editor.recipe.value?.orientationSteps).toBe(3);
    expect(editor.recipe.value?.crop).toEqual({
      x: 0.1,
      y: 0.1,
      width: 0.5,
      height: 0.5,
    });
  });

  it("keeps geometry visible when a color section is bypassed (geometry is never bypassed)", async () => {
    const wrapper = await mountControls();
    const editor = useDevelopEditor();

    editor.setSectionVisible("color", false);
    expect(editor.recipe.value?.sectionVisibility.color).toBe(false);
    expect(wrapper.find('[data-testid="develop-geometry-root"]').exists()).toBe(
      true,
    );
    expect(
      wrapper
        .find('[data-testid="develop-geometry-root"]')
        .attributes("data-bypassed"),
    ).toBe("false");
  });

  it("localizes the geometry control labels", async () => {
    const wrapper = await mountControls();
    const enDevelop = enMessages.develop as Record<string, any>;
    expect(wrapper.get('[data-testid="develop-geometry-title"]').text()).toBe(
      enDevelop.geometry.title,
    );
    wrapper.unmount();

    const de = await mountControls("de");
    const deDevelop = (deMessages as unknown as Record<string, any>).develop;
    expect(de.get('[data-testid="develop-geometry-title"]').text()).toBe(
      deDevelop.geometry.title,
    );
  });

  it("declares the geometry keys in every supported locale", () => {
    for (const [name, messages] of Object.entries(LOCALE_MESSAGES)) {
      const geometry = ((messages as Record<string, any>).develop || {})
        .geometry;
      expect(
        geometry,
        `locale ${name} must declare develop.geometry`,
      ).toBeTruthy();
      for (const key of [
        "title",
        "rotateLeft",
        "rotateRight",
        "flipHorizontal",
        "flipVertical",
        "reset",
        "aspect",
        "aspectFree",
        "aspectOriginal",
        "cropAria",
        "cropHint",
      ]) {
        expect(typeof geometry[key], `locale ${name} geometry.${key}`).toBe(
          "string",
        );
      }
    }
  });
});
