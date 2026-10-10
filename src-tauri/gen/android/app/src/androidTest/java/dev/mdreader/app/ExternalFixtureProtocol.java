package dev.mdreader.app;

import android.os.Build;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.UUID;

/** SDK/Java only: target-provided Kotlin/AndroidX classes are excluded from the
 * test APK and unavailable to its separately launched components. */
public final class ExternalFixtureProtocol {
    private ExternalFixtureProtocol() {}
    public static final String targetPackage = "dev.mdreader.app";
    public static final String senderPackage = "dev.mdreader.app.test";
    public static final String authority = senderPackage + ".external-fixtures";
    public static final List<String> accepted = Collections.unmodifiableList(Arrays.asList(
        "external-view.md", "external-send.md", "external-multiple-one.md", "external-multiple-two.markdown", "external-after-refusal.md"));
    public static final List<String> refused = Collections.unmodifiableList(Arrays.asList(
        "external-refused.txt", "external-invalid-utf8.md", "external-revoked.md"));
    public static final List<String> names;
    public static final String readingName = "external-reading-position.md";
    public static final int readingSections = 420;
    public static final List<String> allNames;
    static {
        List<String> all = new ArrayList<>(accepted); all.addAll(refused);
        names = Collections.unmodifiableList(all);
        List<String> extended = new ArrayList<>(all); extended.add(readingName);
        allNames = Collections.unmodifiableList(extended);
    }
    public static String token(String value) {
        UUID parsed = UUID.fromString(value);
        if (parsed.version() != 4 || !parsed.toString().equals(value)) throw new IllegalArgumentException();
        return value;
    }
    public static boolean emulator() {
        return Build.HARDWARE.contains("ranchu") || Build.HARDWARE.contains("goldfish") ||
            Build.MODEL.contains("Emulator") || Build.MODEL.contains("Android SDK") || Build.PRODUCT.contains("sdk");
    }
    public static String marker(String run, String name) {
        token(run); if (!allNames.contains(name)) throw new IllegalArgumentException();
        return "externalingress" + run.replace("-", "") + allNames.indexOf(name);
    }
    public static int maxBytes(String name) {
        if (!allNames.contains(name)) throw new IllegalArgumentException();
        return readingName.equals(name) ? 64 * 1024 : 1024;
    }
    public static byte[] bytes(String run, String name) {
        if (!allNames.contains(name)) throw new IllegalArgumentException();
        if (name.equals("external-invalid-utf8.md")) return new byte[] {(byte)0xff, (byte)0xfe};
        if (readingName.equals(name)) {
            String marker = marker(run, name);
            StringBuilder body = new StringBuilder("\uFEFF# External reading position acceptance\r\n\r\n" + marker + "\r\n\r\n");
            for (int section = 1; section <= readingSections; section++) {
                String number = Integer.toString(1000 + section).substring(1);
                body.append("## Section ").append(number).append("\r\n\r\nPosition checkpoint ")
                    .append(number).append(". ").append(marker).append("step").append(number).append("\r\n\r\n");
            }
            byte[] bytes = body.toString().getBytes(StandardCharsets.UTF_8);
            if (bytes.length > maxBytes(name)) throw new IllegalStateException();
            return bytes;
        }
        return ("\uFEFF# External synthetic acceptance\r\n\r\n" + marker(run, name) + "\r\n").getBytes(StandardCharsets.UTF_8);
    }
    public static String digest(byte[] bytes) {
        try {
            StringBuilder out = new StringBuilder();
            for (byte value : MessageDigest.getInstance("SHA-256").digest(bytes)) {
                out.append(Character.forDigit((value >>> 4) & 15, 16));
                out.append(Character.forDigit(value & 15, 16));
            }
            return out.toString();
        } catch (NoSuchAlgorithmException impossible) { throw new IllegalStateException(impossible); }
    }
}
