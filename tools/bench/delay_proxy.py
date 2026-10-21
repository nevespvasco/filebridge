"""TCP proxy adding a fixed one-way delay in each direction (RTT = 2 * delay). Test-only."""
import asyncio, sys, time

LISTEN, TARGET, DELAY = int(sys.argv[1]), int(sys.argv[2]), float(sys.argv[3]) / 1000

async def pump(reader, writer):
    q = asyncio.Queue()
    async def rd():
        while data := await reader.read(65536):
            await q.put((time.monotonic() + DELAY, data))
        await q.put((0, None))
    async def wr():
        while True:
            due, data = await q.get()
            if data is None:
                break
            if (d := due - time.monotonic()) > 0:
                await asyncio.sleep(d)
            writer.write(data)
            await writer.drain()
        writer.close()
    await asyncio.gather(rd(), wr(), return_exceptions=True)

async def handle(cr, cw):
    sr, sw = await asyncio.open_connection("127.0.0.1", TARGET)
    await asyncio.gather(pump(cr, sw), pump(sr, cw), return_exceptions=True)

async def main():
    srv = await asyncio.start_server(handle, "127.0.0.1", LISTEN)
    async with srv:
        await srv.serve_forever()

asyncio.run(main())
