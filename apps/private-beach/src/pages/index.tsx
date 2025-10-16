import Head from "next/head";

export default function Home() {
  return (
    <>
      <Head>
        <title>Private Beach</title>
      </Head>
      <main className="min-h-screen bg-slate-950 text-slate-100 flex flex-col items-center justify-center gap-6">
        <h1 className="text-4xl font-semibold">Private Beach Dashboard</h1>
        <p className="text-slate-300">
          UI scaffolding placeholder. Wire this view to Beach Manager APIs and render session tiles here.
        </p>
      </main>
    </>
  );
}
