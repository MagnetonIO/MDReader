package dev.mdreader.transfer

import androidx.core.content.FileProvider

/** A distinct manifest key keeps the narrow reader-copy cache provider separate
 * from the host's selected-file provider and its independent authority. */
class TransferFileProvider : FileProvider()
