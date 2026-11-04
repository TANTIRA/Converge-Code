package converge

import java.net.UnixDomainSocketAddress
import java.nio.ByteBuffer
import java.nio.channels.SocketChannel
import java.nio.charset.StandardCharsets
import java.nio.file.Path

class UhpClient {
    fun endpoint(home: String, sessionId: String): String {
        val os = System.getProperty("os.name").lowercase()
        return if (os.contains("win")) {
            "\\\\.\\pipe\\converge-$sessionId"
        } else {
            "$home/run/$sessionId.sock"
        }
    }

    fun attach(sessionId: String): String {
        return line("session/attach", obj(mapOf("session_id" to sessionId)), 1)
    }

    fun input(sessionId: String, text: String): String {
        return line("session/input", obj(mapOf("session_id" to sessionId, "text" to text)), 2)
    }

    fun selection(sessionId: String, path: String, text: String): String {
        return line(
            "session/selection",
            obj(mapOf("session_id" to sessionId, "path" to path, "text" to text)),
            3,
        )
    }

    fun respond(sessionId: String, allow: Boolean): String {
        return line(
            "permission/respond",
            """{"session_id":${quote(sessionId)},"allow":$allow}""",
            4,
        )
    }

    fun open(socketPath: String, sessionId: String): String {
        val payload = (attach(sessionId) + "\n").toByteArray(StandardCharsets.UTF_8)
        if (socketPath.startsWith("\\\\.\\pipe\\")) {
            java.io.RandomAccessFile(socketPath, "rw").use { pipe ->
                pipe.write(payload)
                return pipe.readLine() ?: ""
            }
        }
        SocketChannel.open(UnixDomainSocketAddress.of(Path.of(socketPath))).use { channel ->
            channel.write(ByteBuffer.wrap(payload))
            val buffer = ByteBuffer.allocate(8192)
            channel.read(buffer)
            buffer.flip()
            return StandardCharsets.UTF_8.decode(buffer).toString()
        }
    }

    private fun line(method: String, params: String, id: Int): String {
        return """{"jsonrpc":"2.0","id":$id,"method":"$method","params":$params}"""
    }

    private fun obj(fields: Map<String, String>): String {
        return fields.entries.joinToString(prefix = "{", postfix = "}") { (key, value) ->
            "${quote(key)}:${quote(value)}"
        }
    }

    private fun quote(text: String): String {
        val escaped = text.replace("\\", "\\\\").replace("\"", "\\\"")
        return "\"$escaped\""
    }
}
