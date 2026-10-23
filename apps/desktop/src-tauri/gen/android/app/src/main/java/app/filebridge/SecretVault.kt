package app.filebridge

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Profiles' secrets on Android (ADR 0004, ADR 0014): each value is encrypted with an AES key
 * that lives in the Android Keystore and never leaves it; only the ciphertext is stored, in the
 * app's private preferences.
 */
class SecretVault(context: Context) {
  private val prefs = context.getSharedPreferences("filebridge-secrets", Context.MODE_PRIVATE)

  fun get(key: String): String? {
    val stored = prefs.getString(key, null) ?: return null
    val bytes = Base64.decode(stored, Base64.NO_WRAP)
    val cipher = Cipher.getInstance(TRANSFORMATION)
    cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, bytes, 0, IV_LEN))
    return String(cipher.doFinal(bytes, IV_LEN, bytes.size - IV_LEN), Charsets.UTF_8)
  }

  fun set(key: String, value: String) {
    val cipher = Cipher.getInstance(TRANSFORMATION)
    cipher.init(Cipher.ENCRYPT_MODE, key())
    val sealed = cipher.iv + cipher.doFinal(value.toByteArray(Charsets.UTF_8))
    prefs.edit().putString(key, Base64.encodeToString(sealed, Base64.NO_WRAP)).apply()
  }

  fun delete(key: String) {
    prefs.edit().remove(key).apply()
  }

  private fun key(): SecretKey {
    val store = KeyStore.getInstance(KEYSTORE).apply { load(null) }
    (store.getKey(ALIAS, null) as? SecretKey)?.let { return it }
    val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
    generator.init(
      KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
        .setKeySize(256)
        .build()
    )
    return generator.generateKey()
  }

  private companion object {
    const val KEYSTORE = "AndroidKeyStore"
    const val ALIAS = "filebridge-secrets"
    const val TRANSFORMATION = "AES/GCM/NoPadding"
    const val IV_LEN = 12
  }
}
