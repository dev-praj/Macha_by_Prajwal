import { createCipheriv, createDecipheriv, randomBytes } from "node:crypto";

// The provider is deliberately separate from persistence so a host KMS can
// replace the environment-backed key source without changing record logic.
export function createEnvironmentKeyProvider(env = process.env) {
  return {
    currentKeyId() {
      const id = env.COPILOT_ENCRYPTION_KEY_ID || "";
      if (!id) throw new Error("Copilot encryption key is not configured");
      return id;
    },
    keyFor(id) {
      let keys;
      try { keys = JSON.parse(env.COPILOT_ENCRYPTION_KEYS || "{}"); }
      catch { throw new Error("Copilot encryption keys are not configured"); }
      const encoded = keys[id];
      if (typeof encoded !== "string") throw new Error("Copilot encryption key is unavailable");
      const key = Buffer.from(encoded, "base64");
      if (key.length !== 32) throw new Error("Copilot encryption key must be 32 bytes");
      return key;
    },
  };
}

function encrypt(key, plaintext, aad) {
  const nonce = randomBytes(12);
  const cipher = createCipheriv("aes-256-gcm", key, nonce);
  cipher.setAAD(Buffer.from(aad));
  const ciphertext = Buffer.concat([cipher.update(plaintext), cipher.final()]);
  return { nonce: nonce.toString("base64"), ciphertext: ciphertext.toString("base64"), tag: cipher.getAuthTag().toString("base64") };
}

function decrypt(key, value, aad) {
  const decipher = createDecipheriv("aes-256-gcm", key, Buffer.from(value.nonce, "base64"));
  decipher.setAAD(Buffer.from(aad));
  decipher.setAuthTag(Buffer.from(value.tag, "base64"));
  return Buffer.concat([decipher.update(Buffer.from(value.ciphertext, "base64")), decipher.final()]);
}

export function encryptCopilotCredential(credential, accountId, keyProvider) {
  const keyId = keyProvider.currentKeyId();
  const dek = randomBytes(32);
  const aad = `macha:copilot:v1:${accountId}`;
  const plaintext = Buffer.from(JSON.stringify(credential));
  try {
    const data = encrypt(dek, plaintext, aad);
    const wrappedKey = encrypt(keyProvider.keyFor(keyId), dek, `${aad}:dek:${keyId}`);
    return { keyId, ...data, wrappedKey };
  } finally {
    plaintext.fill(0);
    dek.fill(0);
  }
}

export function decryptCopilotCredential(record, keyProvider) {
  const aad = `macha:copilot:v1:${record.account_id}`;
  const dek = decrypt(keyProvider.keyFor(record.encryption_key_id), record.wrapped_dek, `${aad}:dek:${record.encryption_key_id}`);
  try {
    return JSON.parse(decrypt(dek, record.encrypted_token, aad).toString("utf8"));
  } finally { dek.fill(0); }
}
