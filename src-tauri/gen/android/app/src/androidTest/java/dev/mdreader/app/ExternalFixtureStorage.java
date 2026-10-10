package dev.mdreader.app;

import android.content.Context;
import android.os.ParcelFileDescriptor;
import android.os.Process;
import android.system.Os;
import android.system.OsConstants;
import android.system.StructStat;
import java.io.File;
import java.io.FileDescriptor;
import java.io.FileOutputStream;

/** Only the Android framework cache base may be an alias. Named children are
 * lstat/no-follow owned objects; held identities fence every bounded operation. */
final class ExternalFixtureStorage {
    private ExternalFixtureStorage() {}
    private static void require(boolean value) { if (!value) throw new IllegalArgumentException(); }
    private static boolean same(StructStat left, StructStat right) {
        return left.st_dev == right.st_dev && left.st_ino == right.st_ino;
    }
    static final class Directory {
        final File path;
        final StructStat identity;
        final Directory parent;
        Directory(File path, StructStat identity, Directory parent) {
            this.path = path; this.identity = identity; this.parent = parent;
        }
        void verify() throws Exception {
            if (parent != null) parent.verify();
            StructStat current = Os.lstat(path.getPath());
            require(OsConstants.S_ISDIR(current.st_mode) && current.st_uid == Process.myUid() && same(identity, current));
            if (parent != null) require(current.st_dev == parent.identity.st_dev && (current.st_mode & 63) == 0);
        }
    }
    static Directory root(Context context, String run, boolean prepare) throws Exception {
        require(ExternalFixtureProtocol.senderPackage.equals(context.getPackageName()));
        ExternalFixtureProtocol.token(run);
        // Android may return /data/user/0 while the held filesystem is /data/data.
        // Canonicalization belongs only to this framework-controlled base.
        File base = context.getCacheDir().getCanonicalFile();
        StructStat identity = Os.lstat(base.getPath());
        require(OsConstants.S_ISDIR(identity.st_mode) && identity.st_uid == Process.myUid());
        Directory cache = new Directory(base, identity, null); cache.verify();
        Directory fixtures = child(cache, "external-incoming-acceptance", prepare, false);
        return child(fixtures, run, prepare, prepare);
    }
    private static Directory child(Directory parent, String name, boolean create, boolean exclusive) throws Exception {
        require(name.equals("external-incoming-acceptance") || ExternalFixtureProtocol.token(name).equals(name));
        parent.verify();
        File path = new File(parent.path, name);
        if (create) {
            boolean made = path.mkdir();
            if (exclusive) require(made);
            if (made) {
                FileDescriptor directory = Os.open(path.getPath(), OsConstants.O_RDONLY | OsConstants.O_NOFOLLOW | OsConstants.O_CLOEXEC, 0);
                try {
                    StructStat held = Os.fstat(directory);
                    require(OsConstants.S_ISDIR(held.st_mode) && held.st_uid == Process.myUid() && held.st_dev == parent.identity.st_dev);
                    parent.verify(); require(same(held, Os.lstat(path.getPath())));
                    Os.fchmod(directory, 448); parent.verify(); require(same(held, Os.lstat(path.getPath())));
                } finally { Os.close(directory); }
            }
        }
        StructStat stat = Os.lstat(path.getPath());
        require(OsConstants.S_ISDIR(stat.st_mode) && stat.st_uid == Process.myUid() && stat.st_dev == parent.identity.st_dev && (stat.st_mode & 63) == 0);
        parent.verify();
        Directory directory = new Directory(path, stat, parent); directory.verify(); return directory;
    }
    static File file(Directory root, String name) throws Exception {
        require(ExternalFixtureProtocol.allNames.contains(name)); root.verify();
        File file = new File(root.path, name);
        StructStat stat = Os.lstat(file.getPath());
        require(OsConstants.S_ISREG(stat.st_mode) && stat.st_uid == Process.myUid() && stat.st_dev == root.identity.st_dev &&
            stat.st_nlink == 1 && stat.st_size <= ExternalFixtureProtocol.maxBytes(name) && (stat.st_mode & 63) == 0);
        root.verify(); return file;
    }
    static void writeNew(Directory root, String name, byte[] bytes) throws Exception {
        require(ExternalFixtureProtocol.allNames.contains(name) && bytes.length <= ExternalFixtureProtocol.maxBytes(name)); root.verify();
        File file = new File(root.path, name);
        FileDescriptor fd = Os.open(file.getPath(), OsConstants.O_WRONLY | OsConstants.O_CREAT | OsConstants.O_EXCL | OsConstants.O_NOFOLLOW | OsConstants.O_CLOEXEC, 384);
        try (FileOutputStream output = new FileOutputStream(fd)) {
            StructStat held = Os.fstat(fd);
            require(OsConstants.S_ISREG(held.st_mode) && held.st_uid == Process.myUid() && held.st_nlink == 1 && held.st_dev == root.identity.st_dev);
            output.write(bytes); output.getFD().sync(); root.verify();
            require(same(held, Os.lstat(file.getPath())));
        }
    }
    static ParcelFileDescriptor openRead(Directory root, String name) throws Exception {
        File file = file(root, name); StructStat expected = Os.lstat(file.getPath());
        FileDescriptor fd = Os.open(file.getPath(), OsConstants.O_RDONLY | OsConstants.O_NOFOLLOW | OsConstants.O_CLOEXEC, 0);
        try {
            StructStat held = Os.fstat(fd);
            require(same(expected, held) && OsConstants.S_ISREG(held.st_mode) && held.st_uid == Process.myUid() && held.st_nlink == 1 && held.st_size <= ExternalFixtureProtocol.maxBytes(name));
            root.verify(); return ParcelFileDescriptor.dup(fd);
        } finally { Os.close(fd); }
    }
}
