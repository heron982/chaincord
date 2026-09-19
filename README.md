# Chaincord

Live chat descentralizado (alpha). Comunidades, canais, E2EE no desenho — o binário ainda é MVP.

Caderno: [`docs/arquitetura.md`](docs/arquitetura.md). Licença: [Apache-2.0](LICENSE).

This is an **alpha** desktop chat. The live channel key still travels in the invite; do not treat it as production E2EE. See [`SECURITY.md`](SECURITY.md).

## O que funciona hoje

- Criar comunidade ou entrar com convite
- Chat em `#general` na LAN; entre redes se o PC de quem criou for alcançável (o app dele é o relé)
- Call 1:1 (WebRTC). Em CGNAT pode falhar até haver TURN próprio
- Dois clientes no mesmo PC: abra o app duas vezes (a segunda pega outra porta)

## O que ainda não é

MLS, erasure, SFU de grupo, coturn da comunidade, nó `--node` 24h. A chave ao vivo ainda vai no convite.

Não há broker MQTT do Chaincord. Quem cria hospeda o hub no app; o convite leva o endereço. Sem essa máquina alcançável, só a mesma rede. TURN OpenRelay ainda é fallback da call 1:1. Detalhe em [`SECURITY.md`](SECURITY.md).

## Desenvolvimento

Precisa de Node.js, Rust e (no Windows) WebView2 + Build Tools.

```bash
npm install
npm test
npm run test:core
npm run tauri:dev
```

Instalador / exe:

```bash
npm run dist
```

Saída padrão do Tauri: `src-tauri/target/release/bundle/` (NSIS + exe). Se existir um disco `D:\` com o layout local de build, os scripts em `scripts/` usam esse disco; caso contrário usam o Cargo/Node do sistema.

## Uso rápido

1. Abre o app
2. **Criar** comunidade ou **Entrar** com o convite
3. Conversar em `#general`

Mesmo Wi-Fi: o convite já leva o IP da LAN. Redes diferentes: o app de quem criou é o relé — os dois saem até essa máquina.
