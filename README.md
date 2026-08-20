# Chaincord

Live chat descentralizado (MVP). Caderno: [`docs/arquitetura.md`](docs/arquitetura.md).

## Para o tester (sem instalar nada)

Quem **gera** o app (você, com Node + Rust neste repo):

```bash
npm install
npm run dist
```

O executável e o instalador saem em `D:\chaincord\`:

- `D:\chaincord\chaincord.exe` — dois cliques, **janela própria**, sem CMD e sem Chrome
- `D:\chaincord\Chaincord_0.1.0_x64-setup.exe` — instalador NSIS (opcional)

O tester:

1. Abre `chaincord.exe`
2. **Criar** comunidade (ou **Entrar** com o convite)
3. Conversa em `#general`

Dois testers no **mesmo Wi-Fi**: o convite já leva o IP da LAN.

Dois testers em **redes diferentes**: o app abre um caminho pela internet (os dois saem; ninguém precisa abrir porta). Chat e sinal da call passam. A call 1:1 ainda depende de STUN e pode falhar em CGNAT até o coturn.

Duas pessoas no **mesmo PC**: abra o exe duas vezes (a segunda pega outra porta). Copie o convite da primeira.

## Desenvolvimento

```bash
npm install
npm run tauri:dev
```

Nesta máquina o disco C: está cheio, então as ferramentas pesadas ficam no D::

- Visual Studio Build Tools: `D:\Tools\VSBuildTools`
- crates do Cargo: `D:\cargo-home`
- build Rust: `D:\cargo-target\chaincord`
- TEMP: `D:\Temp`

`npm run dist` já usa esses caminhos (`scripts/tauri-build.cmd`).

## O que ainda não é

Stack alvo: Rust + Tauri 2 (esta janela). MLS, erasure, SFU e TURN da call vêm depois. Chat entre redes já usa relé de saída. A chave ao vivo ainda vai no convite.
