package dev.mdreader.app;

import android.app.Activity;
import android.content.ClipData;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.net.Uri;
import android.os.Bundle;
import android.os.Process;
import android.os.ParcelFileDescriptor;
import android.os.ResultReceiver;
import android.util.Log;
import org.json.JSONObject;
import java.io.ByteArrayOutputStream;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;

/** Separate test-APK process/UID. SDK/Java only; target dependency classes are
 * not packaged into the independently launched instrumentation APK. */
public final class ExternalFixtureSenderActivity extends Activity {
    private static void require(boolean value) { if (!value) throw new IllegalArgumentException(); }
    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        @SuppressWarnings("deprecation") ResultReceiver receiver = getIntent().getParcelableExtra("reply");
        boolean hostLaunch = getIntent().getBooleanExtra("hostLaunch", false);
        String run = null, operation = null, hostOperationId = null;
        int targetUid = -1;
        boolean completed = false;
        Bundle reply = new Bundle();
        try {
            require(getIntent().getBooleanExtra("syntheticOwnedEmulator", false) && ExternalFixtureProtocol.emulator());
            require(receiver != null || hostLaunch);
            if (hostLaunch) hostOperationId = ExternalFixtureProtocol.token(getIntent().getStringExtra("hostOperationId"));
            require(ExternalFixtureProtocol.senderPackage.equals(getApplicationContext().getPackageName()));
            targetUid = getPackageManager().getApplicationInfo(ExternalFixtureProtocol.targetPackage, 0).uid;
            require(Process.myUid() != targetUid);
            run = ExternalFixtureProtocol.token(getIntent().getStringExtra("run"));
            operation = getIntent().getStringExtra("operation");
            require(Arrays.asList("prepare", "inspect", "view", "send", "multiple", "recover", "invalid", "revoked", "cleanup",
                "prepare-reading", "inspect-reading", "reading", "cleanup-reading").contains(operation));
            boolean reading = operation.endsWith("-reading") || "reading".equals(operation);
            List<String> fixtureNames = reading ? Collections.singletonList(ExternalFixtureProtocol.readingName) : ExternalFixtureProtocol.names;
            boolean prepare = "prepare".equals(operation) || "prepare-reading".equals(operation);
            ExternalFixtureStorage.Directory root = ExternalFixtureStorage.root(this, run, prepare);
            if (prepare) {
                for (String name : fixtureNames) {
                    ExternalFixtureStorage.writeNew(root, name, ExternalFixtureProtocol.bytes(run, name));
                }
            }
            root.verify();
            JSONObject hashes = new JSONObject();
            for (String name : fixtureNames) {
                ByteArrayOutputStream bytes = new ByteArrayOutputStream();
                try (ParcelFileDescriptor.AutoCloseInputStream input = new ParcelFileDescriptor.AutoCloseInputStream(ExternalFixtureStorage.openRead(root, name))) {
                    byte[] buffer = new byte[1024]; int count;
                    while ((count = input.read(buffer)) != -1) { require(bytes.size() + count <= ExternalFixtureProtocol.maxBytes(name)); bytes.write(buffer, 0, count); }
                }
                require(Arrays.equals(bytes.toByteArray(), ExternalFixtureProtocol.bytes(run, name)));
                hashes.put(name, ExternalFixtureProtocol.digest(bytes.toByteArray()));
            }
            if ("view".equals(operation)) send(run, Intent.ACTION_VIEW, ExternalFixtureProtocol.accepted.subList(0, 1), true, targetUid);
            else if ("send".equals(operation)) send(run, Intent.ACTION_SEND, ExternalFixtureProtocol.accepted.subList(1, 2), true, targetUid);
            else if ("multiple".equals(operation)) send(run, Intent.ACTION_SEND_MULTIPLE, ExternalFixtureProtocol.accepted.subList(2, 4), true, targetUid);
            else if ("recover".equals(operation)) send(run, Intent.ACTION_SEND, ExternalFixtureProtocol.accepted.subList(4, 5), true, targetUid);
            else if ("invalid".equals(operation)) send(run, Intent.ACTION_SEND_MULTIPLE, ExternalFixtureProtocol.refused.subList(0, 2), true, targetUid);
            else if ("revoked".equals(operation)) send(run, Intent.ACTION_VIEW, ExternalFixtureProtocol.refused.subList(2, 3), false, targetUid);
            else if ("reading".equals(operation)) send(run, Intent.ACTION_VIEW, fixtureNames, true, targetUid);
            else if ("cleanup".equals(operation) || "cleanup-reading".equals(operation)) {
                for (String name : fixtureNames) require(ExternalFixtureStorage.file(root, name).delete());
                root.verify(); require(root.path.delete()); root.parent.verify();
            } else require(prepare || "inspect".equals(operation) || "inspect-reading".equals(operation));
            reply.putInt("senderUid", Process.myUid()); reply.putInt("targetUid", targetUid);
            reply.putString("hashes", hashes.toString());
            completed = true;
            if (receiver != null) receiver.send(Activity.RESULT_OK, reply);
        } catch (Exception refused) {
            reply.putString("error", "synthetic_sender_failed");
            if (receiver != null) receiver.send(Activity.RESULT_CANCELED, reply);
        } finally {
            if (hostLaunch) {
                // Public synthetic status only: never a body, URI, path or token.
                if (completed) Log.i("MDReaderSyntheticSender", "run=" + run + " operation=" + operation +
                    " operationId=" + hostOperationId + " senderUid=" + Process.myUid() + " targetUid=" + targetUid + " result=ok");
                else Log.i("MDReaderSyntheticSender", "result=synthetic_sender_failed");
            }
            finish();
        }
    }
    private void send(String run, String action, List<String> names, boolean grant, int targetUid) {
        require(!names.isEmpty() && names.size() <= 2 && ExternalFixtureProtocol.allNames.containsAll(names));
        ArrayList<Uri> uris = new ArrayList<>();
        for (String name : names) uris.add(ExternalFixtureProvider.uri(run, name));
        Intent selection = new Intent(action).setPackage(ExternalFixtureProtocol.targetPackage).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        if (grant) selection.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        else {
            // Prove an actual grant followed by revocation of this exact fixture.
            grantUriPermission(ExternalFixtureProtocol.targetPackage, uris.get(0), Intent.FLAG_GRANT_READ_URI_PERMISSION);
            require(checkUriPermission(uris.get(0), -1, targetUid, Intent.FLAG_GRANT_READ_URI_PERMISSION) == PackageManager.PERMISSION_GRANTED);
            revokeUriPermission(uris.get(0), Intent.FLAG_GRANT_READ_URI_PERMISSION);
            require(checkUriPermission(uris.get(0), -1, targetUid, Intent.FLAG_GRANT_READ_URI_PERMISSION) == PackageManager.PERMISSION_DENIED);
        }
        if (Intent.ACTION_VIEW.equals(action)) selection.setDataAndType(uris.get(0), "text/markdown");
        else {
            selection.setType("text/plain");
            if (Intent.ACTION_SEND.equals(action)) selection.putExtra(Intent.EXTRA_STREAM, uris.get(0));
            else selection.putParcelableArrayListExtra(Intent.EXTRA_STREAM, uris);
        }
        ClipData clip = ClipData.newRawUri("Explicit synthetic Markdown", uris.get(0));
        for (int index = 1; index < uris.size(); index++) clip.addItem(new ClipData.Item(uris.get(index)));
        selection.setClipData(clip);
        ResolveInfo resolved = getPackageManager().resolveActivity(selection, PackageManager.MATCH_DEFAULT_ONLY);
        require(resolved != null && "dev.mdreader.app.MainActivity".equals(resolved.activityInfo.name));
        startActivity(selection);
    }
}
