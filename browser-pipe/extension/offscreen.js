const IFRAME_LOAD_TIMEOUT_MS = 15000
const iframes = new Map()

chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
    if (msg.action === 'create_iframe') {
        const { id, url } = msg

        const target = new URL(url)
        target.hash = 'bpipe-' + id

        const iframe = document.createElement('iframe')
        iframe.src = target.href
        iframe.style.cssText = 'position:absolute;width:0;height:0;border:0;'

        let settled = false

        const timer = setTimeout(() => {
            if (settled) return
            settled = true
            iframe.remove()
            iframes.delete(id)
            sendResponse({ ok: false, error: 'Iframe load timed out' })
        }, IFRAME_LOAD_TIMEOUT_MS)

        iframe.onload = () => {
            if (settled) return
            settled = true
            clearTimeout(timer)
            sendResponse({ ok: true })
        }

        iframe.onerror = () => {
            if (settled) return
            settled = true
            clearTimeout(timer)
            iframe.remove()
            iframes.delete(id)
            sendResponse({ ok: false, error: 'Iframe failed to load' })
        }

        iframes.set(id, iframe)
        document.body.appendChild(iframe)
        return true // async sendResponse
    }

    if (msg.action === 'remove_iframe') {
        const { id } = msg
        const iframe = iframes.get(id)
        if (iframe) {
            iframe.remove()
            iframes.delete(id)
        }
        sendResponse({ ok: true })
    }
})
