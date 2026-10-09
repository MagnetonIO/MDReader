package dev.mdreader.app;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.OpenableColumns;
import java.io.File;
import java.io.FileNotFoundException;
import java.io.IOException;
import java.util.List;

/** Narrow SDK-only test FileProvider. Framework URI grants protect this
 * non-exported provider; only eight fixed synthetic fixture names are mapped.
 * No library directory, arbitrary path, write, delete or account grant exists. */
public final class ExternalFixtureProvider extends ContentProvider {
    @Override public boolean onCreate() {
        return getContext() != null && ExternalFixtureProtocol.senderPackage.equals(getContext().getPackageName());
    }
    public static Uri uri(String run, String name) {
        ExternalFixtureProtocol.token(run);
        if (!ExternalFixtureProtocol.names.contains(name)) throw new IllegalArgumentException();
        return new Uri.Builder().scheme("content").authority(ExternalFixtureProtocol.authority)
            .appendPath(run).appendPath(name).build();
    }
    private File selected(Uri uri) {
        List<String> segments = uri.getPathSegments();
        if (!"content".equals(uri.getScheme()) || !ExternalFixtureProtocol.authority.equals(uri.getAuthority()) ||
            segments.size() != 2 || uri.getQuery() != null || uri.getFragment() != null || getContext() == null)
            throw new IllegalArgumentException();
        String run = ExternalFixtureProtocol.token(segments.get(0)), name = segments.get(1);
        if (!ExternalFixtureProtocol.names.contains(name)) throw new IllegalArgumentException();
        File root = new File(getContext().getCacheDir(), "external-incoming-acceptance/" + run);
        File file = new File(root, name);
        try {
            if (!root.getAbsoluteFile().equals(root.getCanonicalFile()) || !file.getAbsoluteFile().equals(file.getCanonicalFile()) ||
                !file.isFile() || file.length() > 1024) throw new IllegalArgumentException();
        } catch (IOException unavailable) { throw new IllegalArgumentException(); }
        return file;
    }
    @Override public Cursor query(Uri uri, String[] projection, String selection, String[] args, String sort) {
        File file = selected(uri);
        String[] columns = projection == null ? new String[] {OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE} : projection;
        if (columns.length > 2) throw new IllegalArgumentException();
        Object[] values = new Object[columns.length];
        for (int index = 0; index < columns.length; index++) {
            if (OpenableColumns.DISPLAY_NAME.equals(columns[index])) values[index] = file.getName();
            else if (OpenableColumns.SIZE.equals(columns[index])) values[index] = file.length();
            else throw new IllegalArgumentException();
        }
        MatrixCursor cursor = new MatrixCursor(columns, 1); cursor.addRow(values); return cursor;
    }
    @Override public String getType(Uri uri) { return selected(uri).getName().endsWith(".txt") ? "text/plain" : "text/markdown"; }
    @Override public ParcelFileDescriptor openFile(Uri uri, String mode) throws FileNotFoundException {
        if (!"r".equals(mode)) throw new FileNotFoundException("read_only_synthetic_fixture");
        return ParcelFileDescriptor.open(selected(uri), ParcelFileDescriptor.MODE_READ_ONLY);
    }
    @Override public Uri insert(Uri uri, ContentValues values) { throw new UnsupportedOperationException(); }
    @Override public int update(Uri uri, ContentValues values, String selection, String[] args) { throw new UnsupportedOperationException(); }
    @Override public int delete(Uri uri, String selection, String[] args) { throw new UnsupportedOperationException(); }
}
