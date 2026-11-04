package converge

class UhpClient {
    fun attach(sessionId: String): String {
        return line("session/attach", """{"session_id":"$sessionId"}""", 1)
    }

    fun input(text: String): String {
        return line("session/input", """{"text":"$text"}""", 2)
    }

    fun selection(text: String): String {
        return line("session/selection", """{"text":"$text"}""", 3)
    }

    fun respond(allow: Boolean): String {
        return line("permission/respond", """{"allow":$allow}""", 4)
    }

    private fun line(method: String, params: String, id: Int): String {
        return """{"jsonrpc":"2.0","id":$id,"method":"$method","params":$params}"""
    }
}
