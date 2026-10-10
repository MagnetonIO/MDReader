package dev.mdreader.app

import android.app.Application
import dev.mdreader.transfer.IncomingMarkdown
import dev.mdreader.transfer.IncomingMarkdownHost

/** Activities attach presentation handlers; this one process owner serializes
 * every durable selected-file queue operation across their recreation. */
class MainApplication : Application(), IncomingMarkdownHost {
    override val incomingMarkdown: IncomingMarkdown by lazy(LazyThreadSafetyMode.SYNCHRONIZED) {
        IncomingMarkdown(this)
    }
}
