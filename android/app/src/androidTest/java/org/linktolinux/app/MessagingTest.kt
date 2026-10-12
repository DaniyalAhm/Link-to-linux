package org.linktolinux.app

import android.content.ContentValues
import android.net.Uri
import android.os.Build
import android.provider.Telephony
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Test
import org.junit.Assert.*
import com.google.android.mms.pdu_alt.PduParser
import com.google.android.mms.pdu_alt.SendReq

/** Emulator-only provider fixtures; never submits a message to a carrier. */
class MessagingTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    @Test fun testGroupPduPreservesRecipientsAndUnicode() {
        val recipients = listOf("+15550109123", "+15550109124")
        val text = "Group message with Unicode: café 👋"
        val bytes = GroupMms.compose(instrumentation.targetContext, recipients, text)
        val decoded = PduParser(bytes).parse() as SendReq
        assertEquals(recipients, decoded.to.map { it.string.substringBefore('/') })
        assertEquals(text, String(decoded.body.getPart(0).data, Charsets.UTF_8))
    }

    @Test fun testContactSearchAndNames() {
        check(Build.HARDWARE in setOf("ranchu", "goldfish")) { "Contact fixtures require an emulator" }
        val context = instrumentation.targetContext
        val resolver = context.contentResolver
        instrumentation.uiAutomation.adoptShellPermissionIdentity("android.permission.WRITE_CONTACTS", "android.permission.READ_CONTACTS")
        var raw: Uri? = null
        try {
            raw = resolver.insert(android.provider.ContactsContract.RawContacts.CONTENT_URI, ContentValues())!!
            val id = android.content.ContentUris.parseId(raw)
            resolver.insert(android.provider.ContactsContract.Data.CONTENT_URI, ContentValues().apply {
                put("raw_contact_id", id); put("mimetype", "vnd.android.cursor.item/name"); put("data1", "Link Fixture Contact")
            })
            resolver.insert(android.provider.ContactsContract.Data.CONTENT_URI, ContentValues().apply {
                put("raw_contact_id", id); put("mimetype", "vnd.android.cursor.item/phone_v2"); put("data1", "+15550109123"); put("data2", 2)
            })
            val store = MessagingStore(context)
            val contacts = store.contacts("Link Fixture", 0).getJSONArray("contacts")
            assertTrue(contacts.length() > 0)
            assertEquals("Link Fixture Contact", contacts.getJSONObject(0).getString("name"))
            assertEquals("Link Fixture Contact", store.name("+15550109123"))
        } finally {
            raw?.let { resolver.delete(it, null, null) }
            instrumentation.uiAutomation.dropShellPermissionIdentity()
        }
    }

    @Test fun testSmsAndMmsHistoryPaginationAndGrouping() {
        check(Build.HARDWARE in setOf("ranchu", "goldfish")) { "Provider fixtures require an emulator" }
        val context = instrumentation.targetContext
        val resolver = context.contentResolver
        val created = mutableListOf<Uri>()
        val recipients = setOf("+15550109123", "+15550109124")
        val group = Telephony.Threads.getOrCreateThreadId(context, recipients)
        val base = 1700000000000L
        try {
            repeat(105) { index ->
                created.add(resolver.insert(Telephony.Sms.CONTENT_URI, ContentValues().apply {
                    put("thread_id", group); put("address", recipients.first()); put("body", "History fixture $index")
                    put("date", base + index * 1000); put("type", if (index % 2 == 0) 1 else 2); put("read", 1)
                })!!)
            }
            val mms = resolver.insert(Telephony.Mms.CONTENT_URI, ContentValues().apply {
                put("thread_id", group); put("date", (base + 105000) / 1000); put("msg_box", 1)
                put("m_type", 132); put("ct_t", "application/vnd.wap.multipart.mixed"); put("read", 1)
            })!!
            created.add(mms)
            resolver.insert(Uri.withAppendedPath(mms, "addr"), ContentValues().apply {
                put("address", recipients.last()); put("type", 137); put("charset", 106)
            })
            resolver.insert(Uri.withAppendedPath(mms, "part"), ContentValues().apply {
                put("ct", "text/plain"); put("text", "Group MMS fixture"); put("seq", 0); put("chset", 106)
            })
            val store = MessagingStore(context)
            assertEquals(recipients, store.recipients(group).toSet())
            val newest = store.history(group, 0)
            assertEquals(100, newest.getJSONArray("messages").length())
            assertEquals(100, newest.getInt("next_offset"))
            val messages = newest.getJSONArray("messages")
            assertEquals("Group MMS fixture", messages.getJSONObject(99).getString("body"))
            assertEquals(recipients.last(), messages.getJSONObject(99).getString("address"))
            val older = store.history(group, 100)
            assertEquals(6, older.getJSONArray("messages").length())
            assertTrue(older.isNull("next_offset"))
            assertEquals("History fixture 0", older.getJSONArray("messages").getJSONObject(0).getString("body"))
        } finally {
            created.asReversed().forEach { resolver.delete(it, null, null) }
        }
    }
}
