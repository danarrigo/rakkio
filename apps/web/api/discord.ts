import { waitUntil } from '@vercel/functions'
import { receiveDiscord } from '../server/discord-relay.js'

export default {
  fetch(request: Request) {
    return receiveDiscord(
      request,
      {
        applicationId: process.env.DISCORD_APPLICATION_ID ?? '',
        publicKey: process.env.DISCORD_PUBLIC_KEY ?? '',
        backendUrl: process.env.DISCORD_BACKEND_URL ?? '',
      },
      waitUntil,
    )
  },
}
