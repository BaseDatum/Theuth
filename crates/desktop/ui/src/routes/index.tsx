import { createFileRoute } from '@tanstack/react-router'
import { useEffect } from 'react'
import { Composer } from '@/components/chat/Composer'
import { Header } from '@/components/chat/Header'

export const Route = createFileRoute('/')({ component: Index })

/** New chat: nothing exists until the first `send_input(null, …)` reply names the session. */
function Index() {
  useEffect(() => {
    document.title = ''
  }, [])
  return (
    <>
      <Header session={null} />
      <div className="flex min-h-0 flex-1 flex-col justify-center pb-24">
        <h1 className="mb-6 text-center text-[28px] leading-9 font-semibold">Where should we begin?</h1>
        <Composer session={null} />
      </div>
    </>
  )
}
