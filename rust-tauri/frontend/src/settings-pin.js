export const SAVED_PIN_MASK = "••••••";

export function encryptionPinSubmission(value, keepSavedPin) {
  return keepSavedPin ? SAVED_PIN_MASK : value;
}

export function configureArguments(form) {
  return [
    form.repoURL,
    form.serverURL,
    form.username,
    form.token,
    form.useAPI,
    form.interval,
    form.keepToken,
    form.encryptionPin,
  ];
}

export function encryptionPinFingerprint(value, keepSavedPin) {
  return keepSavedPin ? "KEEP_SAVED_PIN" : value;
}

export function isEncryptionPinValid(value, keepSavedPin) {
  const pin = encryptionPinSubmission(value, keepSavedPin);
  return pin === SAVED_PIN_MASK || pin === "" || /^[0-9]{6}$/.test(pin);
}

export function shouldKeepSavedPin(value, keepSavedPin) {
  return keepSavedPin && value === SAVED_PIN_MASK;
}

export function decryptionErrorMessage(message) {
  const error = String(message || "");
  return `Sync failed: ${error}`;
}

export function pinLockMessage(lockUntil, nowSeconds) {
  return lockUntil > nowSeconds
    ? `PIN verification is locked for ${lockUntil - nowSeconds} seconds.`
    : "";
}

export function canSaveSettings(validatedFingerprint, currentFingerprint) {
  return Boolean(validatedFingerprint) && validatedFingerprint === currentFingerprint;
}

export function settingsActions(stage) {
  return {
    validate: stage === "editing",
    cancel: stage === "editing",
    save: stage === "validated",
    editable: stage === "editing",
    closable: stage === "editing",
  };
}

export function settingsMayClose(stage, saved) {
  return stage === "editing" || (stage === "validated" && saved);
}

export function settingsStageAfterValidation(ok) {
  return ok ? "validated" : "editing";
}

export async function runSettingsValidation({ unlock, configure, validate }) {
  await unlock();
  await configure();
  return validate();
}
