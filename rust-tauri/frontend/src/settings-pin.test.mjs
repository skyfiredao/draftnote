import assert from "node:assert/strict";
import test from "node:test";
import {
  SAVED_PIN_MASK,
  configureArguments,
  canSaveSettings,
  decryptionErrorMessage,
  encryptionPinFingerprint,
  encryptionPinSubmission,
  isEncryptionPinValid,
  pinLockMessage,
  runSettingsValidation,
  settingsActions,
  settingsMayClose,
  settingsStageAfterValidation,
  shouldKeepSavedPin,
} from "./settings-pin.js";

test("saved PIN stays masked and submits the retain sentinel", () => {
  assert.equal(SAVED_PIN_MASK, "••••••");
  assert.equal(encryptionPinSubmission(SAVED_PIN_MASK, true), SAVED_PIN_MASK);
  assert.equal(encryptionPinFingerprint(SAVED_PIN_MASK, true), "KEEP_SAVED_PIN");
  assert.equal(isEncryptionPinValid(SAVED_PIN_MASK, true), true);
});

test("edited PIN replaces saved PIN; empty PIN clears encryption", () => {
  assert.equal(encryptionPinSubmission("654321", false), "654321");
  assert.equal(encryptionPinSubmission("", false), "");
  assert.equal(encryptionPinFingerprint("654321", false), "654321");
  assert.equal(isEncryptionPinValid("654321", false), true);
  assert.equal(isEncryptionPinValid("12345", false), false);
  assert.equal(shouldKeepSavedPin(SAVED_PIN_MASK, true), true);
  assert.equal(shouldKeepSavedPin("654321", true), false);
  assert.equal(shouldKeepSavedPin("", true), false);
});

test("Configure receives settings fields in the Rust command's positional order", () => {
  assert.deepEqual(configureArguments({
    repoURL: "repo",
    serverURL: "server",
    username: "user",
    token: "token",
    useAPI: true,
    interval: 15,
    keepToken: false,
    encryptionPin: "123456",
  }), ["repo", "server", "user", "token", true, 15, false, "123456"]);
});

test("background errors report the failure without opening PIN flow", () => {
  assert.equal(decryptionErrorMessage("incorrect PIN"), "Sync failed: incorrect PIN");
  assert.equal(decryptionErrorMessage("network unavailable"), "Sync failed: network unavailable");
  assert.equal(pinLockMessage(1300, 1000), "PIN verification is locked for 300 seconds.");
  assert.equal(pinLockMessage(900, 1000), "");
});

test("Save is enabled only for an unchanged validated settings fingerprint", () => {
  assert.equal(canSaveSettings(null, "x"), false);
  assert.equal(canSaveSettings("x", "y"), false);
  assert.equal(canSaveSettings("x", "x"), true);
});

test("masked saved PIN is retained through settings submission", () => {
  assert.equal(shouldKeepSavedPin(SAVED_PIN_MASK, true), true);
  assert.equal(shouldKeepSavedPin("", false), false);
  assert.equal(shouldKeepSavedPin(SAVED_PIN_MASK, false), false);
});

test("Settings actions follow editing, validation, and save-only stages", () => {
  assert.deepEqual(settingsActions("editing"), { validate: true, cancel: true, save: false, editable: true, closable: true });
  assert.deepEqual(settingsActions("validating"), { validate: false, cancel: false, save: false, editable: false, closable: false });
  assert.deepEqual(settingsActions("validated"), { validate: false, cancel: false, save: true, editable: false, closable: false });
});

test("Cancel and backdrop cannot close after validation starts", () => {
  assert.equal(settingsMayClose("editing", false), true);
  assert.equal(settingsMayClose("validating", false), false);
  assert.equal(settingsMayClose("validated", false), false);
  assert.equal(settingsMayClose("validated", true), true);
});

test("Validate unlocks, configures, then fully synchronizes in order", async () => {
  const order = [];
  const result = await runSettingsValidation({
    unlock: async () => { order.push("unlock"); },
    configure: async () => { order.push("configure"); },
    validate: async () => { order.push("full sync"); return "OK"; },
  });
  assert.deepEqual(order, ["unlock", "configure", "full sync"]);
  assert.equal(result, "OK");
});

test("failed Configure leaves Settings cancellable without a sync or rollback", async () => {
  const order = [];
  await assert.rejects(runSettingsValidation({
    unlock: async () => { order.push("unlock"); },
    configure: async () => { order.push("configure"); throw new Error("invalid"); },
    validate: async () => { order.push("full sync"); },
  }), /invalid/);
  assert.deepEqual(order, ["unlock", "configure"]);
  assert.equal(settingsActions("editing").cancel, true);
});

test("failed full sync cannot enter Save-only stage", async () => {
  const order = [];
  await assert.rejects(runSettingsValidation({
    unlock: async () => { order.push("unlock"); },
    configure: async () => { order.push("configure"); },
    validate: async () => { order.push("full sync"); throw new Error("sync failed"); },
  }), /sync failed/);
  assert.deepEqual(order, ["unlock", "configure", "full sync"]);
  assert.equal(settingsActions("editing").save, false);
  assert.equal(settingsStageAfterValidation(false), "editing");
  assert.equal(settingsStageAfterValidation(true), "validated");
});
