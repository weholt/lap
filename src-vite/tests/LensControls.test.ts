import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount } from "@vue/test-utils";
import { createI18n } from "vue-i18n";

// Component regressions for the lens-correction controls (lap-d52 /
// TASK-502). Spec refs A7/A9/A10: versioned profile resources with visible
// provenance, explicit capability errors for missing/unsupported profiles,
// and never-silent correction changes.

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

const dialogOpenMock = vi.fn();

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: (...args: unknown[]) => dialogOpenMock(...args),
}));

import LensControls from "@/components/develop/LensControls.vue";
import {
  DEFAULT_RECIPE,
  RECIPE_SCHEMA_VERSION,
  type LensDistortionParams,
  type Recipe,
} from "@/composables/useDevelopSession.types";
import enMessages from "@/locales/en.json";

function makeI18n() {
  return createI18n({
    legacy: false,
    locale: "en",
    fallbackLocale: "en",
    messages: { en: enMessages as Record<string, unknown> },
  });
}

const appliedPatches: Array<{ patch: Partial<Recipe>; label: string }> = [];

function applyPatch(patch: Partial<Recipe>, label: string) {
  appliedPatches.push({ patch, label });
}

const PROFILE_SUMMARY = {
  id: `lens/${"a".repeat(64)}`,
  lensCount: 2,
  cameraCount: 1,
};

const RESOLVED_PARAMS: LensDistortionParams = {
  k1: -0.02,
  k2: 0.01,
  k3: 0.002,
  model: 0,
  tca_vr: 1.0004,
  tca_vb: 0.9998,
  vig_k1: -0.2,
  vig_k2: 0.05,
  vig_k3: 0.01,
};

const PROFILE_REF = {
  uri: `resource://lens/${"a".repeat(64)}`,
  maker: "TestCorp Optics",
  model: "Test 24-70mm f/2.8",
  version: "lensfun 2020-01-01",
  sha256: "a".repeat(64),
};

function mountControls(recipe: Recipe = structuredClone(DEFAULT_RECIPE)) {
  return mount(LensControls, {
    props: {
      disabled: false,
      active: true,
      recipe,
      applyPatch,
    },
    global: {
      plugins: [makeI18n()],
    },
  });
}

beforeEach(() => {
  invokeMock.mockReset();
  dialogOpenMock.mockReset();
  appliedPatches.length = 0;
  invokeMock.mockImplementation((command: string) => {
    if (command === "develop_lens_catalog") return Promise.resolve([PROFILE_SUMMARY]);
    if (command === "develop_lens_makers")
      return Promise.resolve(["TestCorp Optics", "OtherCorp"]);
    if (command === "develop_lens_models") return Promise.resolve(["Test 24-70mm f/2.8"]);
    return Promise.resolve(undefined);
  });
});

afterEach(() => {
  vi.restoreAllMocks();
});

async function selectLens(wrapper: ReturnType<typeof mountControls>) {
  await flushPromises();
  await wrapper.find('[data-testid="develop-lens-profile"]').setValue(PROFILE_SUMMARY.id);
  await flushPromises();
  await wrapper.find('[data-testid="develop-lens-maker"]').setValue("TestCorp Optics");
  await flushPromises();
  await wrapper.find('[data-testid="develop-lens-model"]').setValue("Test 24-70mm f/2.8");
  await flushPromises();
}

describe("LensControls", () => {
  it("shows the visible no-profiles capability message when nothing is imported", async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === "develop_lens_catalog") return Promise.resolve([]);
      return Promise.resolve(undefined);
    });
    const wrapper = mountControls();
    await flushPromises();
    expect(wrapper.find('[data-testid="develop-lens-no-profiles"]').exists()).toBe(true);
    const text = wrapper.text();
    expect(text).toContain("No lens profiles imported");
  });

  it("selects a lens and applies provenance, params and mode through one patch", async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === "develop_select_lens")
        return Promise.resolve({
          params: RESOLVED_PARAMS,
          notices: [{ kind: "partial-tca-model", detail: "cubic coefficients ignored" }],
          profile: PROFILE_REF,
        });
      if (command === "develop_lens_catalog") return Promise.resolve([PROFILE_SUMMARY]);
      if (command === "develop_lens_makers")
        return Promise.resolve(["TestCorp Optics", "OtherCorp"]);
      if (command === "develop_lens_models") return Promise.resolve(["Test 24-70mm f/2.8"]);
      return Promise.resolve([]);
    });

    const wrapper = mountControls();
    await selectLens(wrapper);
    await wrapper.find('[data-testid="develop-lens-apply"]').trigger("click");
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith(
      "develop_select_lens",
      expect.objectContaining({
        profileId: PROFILE_SUMMARY.id,
        maker: "TestCorp Optics",
        model: "Test 24-70mm f/2.8",
        focalLength: 50,
      }),
    );
    expect(appliedPatches).toHaveLength(1);
    expect(appliedPatches[0].label).toBe("lens-select");
    expect(appliedPatches[0].patch.lensProfile).toEqual(PROFILE_REF);
    expect(appliedPatches[0].patch.lensDistortionParams).toEqual(RESOLVED_PARAMS);
    expect(appliedPatches[0].patch.lensMaker).toBe("TestCorp Optics");
    // Capability notices stay visible, never swallowed.
    const notices = wrapper.find('[data-testid="develop-lens-notices"]');
    expect(notices.exists()).toBe(true);
    expect(notices.text()).toContain("cubic coefficients ignored");
  });

  it("surfaces unsupported-lens failures visibly and applies no patch", async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === "develop_select_lens")
        return Promise.reject("lens profile cannot be used: unsupported lens distortion model 'ptbrown' for LegacyCorp Legacy 50mm f/2");
      if (command === "develop_lens_catalog") return Promise.resolve([PROFILE_SUMMARY]);
      if (command === "develop_lens_makers")
        return Promise.resolve(["TestCorp Optics", "OtherCorp"]);
      if (command === "develop_lens_models") return Promise.resolve(["Test 24-70mm f/2.8"]);
      return Promise.resolve([]);
    });

    const wrapper = mountControls();
    await selectLens(wrapper);
    await wrapper.find('[data-testid="develop-lens-apply"]').trigger("click");
    await flushPromises();

    expect(appliedPatches).toHaveLength(0);
    const error = wrapper.find('[data-testid="develop-lens-error"]');
    expect(error.exists()).toBe(true);
    expect(error.text()).toContain("ptbrown");
  });

  it("imports profiles through the dialog and refreshes the catalog", async () => {
    dialogOpenMock.mockResolvedValue(["C:/lensfun/mil-sony.xml"]);
    invokeMock.mockImplementation((command: string) => {
      if (command === "develop_lens_catalog") {
        return Promise.resolve(
          invokeMock.mock.calls.filter(([cmd]) => cmd === "develop_import_lens_profile").length > 0
            ? [PROFILE_SUMMARY, { id: `lens/${"b".repeat(64)}`, lensCount: 9, cameraCount: 3 }]
            : [PROFILE_SUMMARY],
        );
      }
      if (command === "develop_import_lens_profile")
        return Promise.resolve([
          { id: `lens/${"b".repeat(64)}`, digest: "b".repeat(64), sizeBytes: 10, lensCount: 9, cameraCount: 3, version: "lensfun 2020-01-01", newlyStored: true },
        ]);
      if (command === "develop_lens_makers")
        return Promise.resolve(["TestCorp Optics", "OtherCorp"]);
      if (command === "develop_lens_models") return Promise.resolve(["Test 24-70mm f/2.8"]);
      return Promise.resolve([]);
    });

    const wrapper = mountControls();
    await flushPromises();
    await wrapper.find('[data-testid="develop-lens-import"]').trigger("click");
    await flushPromises();

    expect(dialogOpenMock).toHaveBeenCalled();
    expect(invokeMock).toHaveBeenCalledWith(
      "develop_import_lens_profile",
      expect.objectContaining({
        paths: ["C:/lensfun/mil-sony.xml"],
        version: "local snapshot (unversioned)",
      }),
    );
    const options = wrapper.findAll('[data-testid="develop-lens-profile"] option');
    expect(options.length).toBeGreaterThanOrEqual(2);
  });

  it("toggles correction components and amount edits through the recipe patch", async () => {
    const wrapper = mountControls();
    await flushPromises();

    await wrapper.find('[data-testid="develop-lens-distortion-enabled"]').setValue(false);
    await flushPromises();
    expect(appliedPatches.at(-1)?.patch.lensDistortionEnabled).toBe(false);

    await wrapper.find('[data-testid="develop-lens-vignette-enabled"]').setValue(false);
    await flushPromises();
    expect(appliedPatches.at(-1)?.patch.lensVignetteEnabled).toBe(false);

    await wrapper.find('[data-testid="develop-lens-tca-enabled"]').setValue(false);
    await flushPromises();
    expect(appliedPatches.at(-1)?.patch.lensTcaEnabled).toBe(false);
  });

  it("shows existing provenance from the opened recipe", async () => {
    const recipe = structuredClone(DEFAULT_RECIPE);
    recipe.lensProfile = PROFILE_REF;
    const wrapper = mountControls(recipe);
    await flushPromises();
    const provenance = wrapper.find('[data-testid="develop-lens-provenance"]');
    expect(provenance.text()).toContain("lensfun 2020-01-01");
  });

  it("renders localized labels without missing keys", async () => {
    const wrapper = mountControls();
    await flushPromises();
    expect(wrapper.find('[data-testid="develop-lens-title"]').text()).toContain("Lens Correction");
  });
});
