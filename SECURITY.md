# Segurança

O Chaincord está em **alpha**. Não use para dado que não poderia ir num grupo cujo convite vazou.

## O que vale hoje

- A chave ao vivo do canal vai **no convite**. Quem tem o convite lê o chat.
- Não há broker MQTT do Chaincord. Quem **cria** a comunidade hospeda o hub no próprio app; o convite leva o `ws://`. Quem entra não configura nada. Sem o criador alcançável (NAT sem porta / sem VPS), o chat entre redes não sobe. LAN e P2P direto continuam. O tópico inclui o `community_id`; o hub vê metadados, não o texto (se a chave do convite estiver certa).
- `CHAINCORD_RELAY=off` desliga o cliente MQTT neste processo. `CHAINCORD_RELAY=wss://…` na criação troca o hub local por um broker externo.
- TURN `openrelay.metered.ca` e STUN públicos existem só para a call 1:1 atravessar NAT. Não são infra de produção.
- O desenho alvo (MLS, erasure, nó cego, SFU da comunidade) está em [`docs/arquitetura.md`](docs/arquitetura.md) e **ainda não está no binário**.

## Relé próprio

Não há campo na UI. O desktop que cria a comunidade já é o hub. Para internet, essa máquina precisa aceitar TCP na porta do app (LAN, VPS, ou porta aberta).

## Relatar falha

Não abra issue pública com exploit, dump de chave ou PoC.

Preferência: [GitHub Security Advisories](https://github.com/heron982/chaincord/security/advisories/new) (Private vulnerability reporting).

Ou e-mail: `felipedevlp@gmail.com`, com:

- versão (`package.json` / `src-tauri/tauri.conf.json`)
- o que acontece e o que deveria acontecer
- se a falha deixa um terceiro ler mensagem, fingir identidade ou derrubar a rede
