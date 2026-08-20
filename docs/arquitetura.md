# Chaincord — caderno de arquitetura

Documento vivo da discussão de produto e arquitetura.

Vocabulário do projeto: **comunidade**, **log assinado**, **canal**, **nó**, **cliente**, **E2EE**. Não usar analogias nem implementação de ledger.

- **Definições (Felipe)** — o que você fechou. Só muda se você redefinir.
- **Aberto** — hipóteses e pontos em disputa.
- **Revisão técnica** — análise do arquiteto; não vira definição sozinha.

Quando algo for definido na conversa, entra na seção de definições e no histórico no fim.

---

## Definições (Felipe)

1. **A rede deve ser descentralizada.** A infra de comunicação não depende de um operador central como dono da rede. Usuários (e nós que eles controlam) são a base da disponibilidade.
2. A plataforma é um **live chat** com modelo mental semelhante ao Discord: comunidades, canais, roles, permissions.
3. A comunicação deve ser **E2EE**. Peers intermediários veem ciphertext, não o conteúdo.
4. **Mensagens vivem no log do canal**, não no log de autoridade da comunidade.
5. **Não assumir** que: todo usuário precisa hospedar dados; toda comunicação precisa ser P2P direto; não pode existir nenhum servidor; token/recompensa é obrigatório; DAO é obrigatório; toda comunidade precisa ser pública.
6. Comunidades podem ser **públicas ou privadas**. Privadas não devem ser necessariamente enumeráveis. Usuários podem ser **pseudônimos**. Membros não devem ser globalmente rastreáveis por padrão.
7. Dados privados permanecem nos nós da comunidade e nos clientes, cifrados. Não há registro público de membership nem de relação entre usuários.
8. **Autoridade da comunidade ≠ infra que entrega os bytes.** Um nó pode hospedar sem possuir a comunidade.
9. Infra própria da plataforma, se existir, é para **coordenação e fallback**, não para ser o caminho principal por desenho.
10. Tokenomics / recompensas **não são requisito**, a menos que resolvam um problema técnico concreto.
11. O que se “possui” é a **comunidade** (dono, cargos, permissões). Mensagem não é esse estado.
12. Cada comunidade tem o seu **log assinado** (ops a partir de um genesis). É a fonte de verdade de autoridade.
13. **Fora de escopo:** qualquer modelo, analogia ou implementação de ledger. Daqui pra frente o caderno e a conversa não usam esses termos como ferramenta de desenho.
14. **Membro novo vê o histórico do canal.** Comportamento Discord: entrar na comunidade é ver o passado de `#general`, não começar do zero.
15. **O nó da comunidade não guarda chaves de histórico e não lê o canal.** Se não houver nenhum membro antigo online no join, o cliente novo **não decifra o passado** até um membro antigo aparecer e compartilhar as chaves. O nó pode já ter o ciphertext; sem chave, é ilegível.
16. **Sem planos de armazenamento comerciais.** Quem precisa de histórico persistente opera o próprio nó (PC/VPS) e assume o disco. Fora de escopo cobrar GB/retenção como produto da plataforma.
17. **Histórico da comunidade é dividido entre os membros daquela comunidade** (sharding / erasure code). Não há um único HD dono do arquivo, nem swarm entre usuários de outras comunidades. Cada membro do conjunto de storage guarda **pedaços cifrados**; com k de n pedaços dá para reconstruir. Sem as chaves do canal, o pedaço continua ilegível (definição 15).
20. **Storage peer não some com a parcela em silêncio.** Quem está no *n* do erasure só deixa o papel de storage (ou a comunidade) depois que a parcela foi **recolocada**. Preferência do Felipe: na saída, **escolher outro membro elegível** para assumir. O app também pode **reparar sozinho** no pool que restou (não obrigar a escolher se o pool aguenta). Destino tem que ser storage-capaz (mesmo critério de “pode persistir”, não celular por padrão) e aceitar cota. Kick: reparo **sem** o chutado escolher. Crash / formatar / sumir: não há diálogo — vale definição 18 (pendente ou perdido). Com a def. 22 não há “light só-cache” na guild: sair sempre passa pela regra de parcela (reparo automático se o pool aguentar).
18. **Chat ao vivo não depende de histórico.** O produto aceita que o passado pode ficar **pendente** (sem chave, ou sem k pedaços online). A UI fala isso sem fingir Discord quando a guild está vazia. Offline ≠ perda se o pedaço está no disco de alguém; perda de verdade só se os pedaços sumiram (formatou, saiu, HD morreu e o n não cobre).
19. **Modo da call pelo número de clientes.** **2 clientes → 1:1** (WebRTC entre os dois + TURN se NAT; **sem SFU**), mesmo que os dois sejam celular. **3 ou mais → elege uma SFU** automaticamente no pool da comunidade. O usuário não configura servidor. Não picotar o stream entre SFUs. Se o eleito cair, reelege outro. Se 3+ e ninguém for elegível, câmera/tela de grupo fica indisponível (a call 1:1 de 2 pessoas não depende disso).

Se um terceiro entra numa call que já era 1:1, o app **migra** para SFU (se houver elegível) ou recusa o vídeo do terceiro / deixa o ícone indisponível. Se a sala volta a 2, pode voltar a 1:1.

**Não pode ser SFU (por padrão):**
- celular (iOS/Android);
- dispositivo só em dados móveis (4G/5G);
- cliente em background / economia de energia;
- quem falha o teste automático (upload insuficiente, NAT inalcançável sem ser o caso TURN-only inviável para hub, CPU/térmico).

**Pode ser SFU:** desktop (Windows/macOS/Linux) na call, Wi-Fi/Ethernet, app em primeiro plano, upload acima do piso da sala. VPS de um membro conta como desktop sempre ligado.

**UI:** na call com 3+, um ícone no tile do participante deixa **explícito quem é a SFU**. Se reeleger, o ícone muda de pessoa. Em 1:1 (2 clientes) o ícone **não aparece** — não há SFU.

Implicação 19: todo mundo é cliente; no máximo **um** device da call é hub. Mesh “todos SFU” e fatiar frames entre máquinas ficam fora. TURN da plataforma continua invisível para atravessar NAT até a SFU eleita. Gravacao da call no erasure: fora.

Implicação 14+15: canais de comunidade não têm forward secrecy contra membros futuros, mas quem só guarda pedaço **não é participante da criptografia**. UX diferente do Discord no canto vazio: entrar numa guild morta mostra canal sem passado (ou “histórico pendente”) até alguém da comunidade ligar. DMs continuam mais estritas.

Implicação 17+22: o pool é **por comunidade**, não o Chaincord inteiro. **Todo cliente da guild entra no *n*** (pedaços cifrados); celular com cota menor, não zero. Owner / primeiro device **não** é o servidor único. Kick/saída: handoff (def. 20) para qualquer um que saia, inclusive mobile. Comunidade só de mobile continua frágil (devices dormem, pouco disco).

Implicação 18: não se promete “entrou, leu 2019” se não houver chave nem k storage peers. Promete-se: a conversa de agora funciona; o arquivo reaparece quando a comunidade acorda. UI distingue os três estados abaixo.

21. **Stack do MVP:** núcleo em **Rust** (identidade, log, MLS, erasure, sync, SFU no desktop); UI desktop **Tauri 2 + React/TS**; cache **SQLite**; chat **WebSocket** (QUIC depois); call 1:1 **WebRTC**; SFU embutida **`str0m`**; NAT **coturn**; erasure **`reed-solomon-erasure`**; grupo E2EE **OpenMLS**; identidade **ed25519-dalek** + `did:key`. O mesmo binário Rust com flag `--node` cobre VPS/PC 24h. Mobile light depois (UniFFI). Fora do v1: Electron, DHT/libp2p, fork Matrix, LiveKit/mediasoup como produto, cripto na UI.
22. **Todo cliente da comunidade é nó de histórico, não só de mensagem ao vivo.** Entrar na guild = entrar no *n* do erasure daquela comunidade (pedaços cifrados). Não existe “só conversa, HD é problema de outro”. O que **não** é fixo: um único device dono do arquivo (owner / primeiro a abrir **não** é *o* servidor). O arquivo é o conjunto dos clientes. Cota por device (celular guarda menos que desktop); handoff na saída continua (def. 20). Sem *k* pedaços recuperáveis, histórico pendente/perdido (def. 18). Chat ao vivo não espera o arquivo.

### Hipótese (ainda não fechada)

- **Auditoria de mensagens** — incluir alguma prova no log de autoridade? Recomendação da revisão: corpo não; log por canal; Merkle root no log de autoridade só se virar requisito. Ver [Histórico e auditoria](#histórico-e-auditoria).

---

## Aberto

| Tópico | Posição do Felipe | Posição da revisão | Status |
|---|---|---|---|
| Rede descentralizada | Exigência | Log assinado + nós substituíveis | **Definido** |
| Auditoria de mensagens no log de autoridade | Em dúvida | Corpo não; hash chain por canal | **Aberto** |
| P2P como caminho primário de entrega | Intenção original | P2P = atalho + sync entre réplicas; entrega ao vivo pelos nós da comunidade | Em disputa |
| Quem opera storage | Todos os clientes daquela comunidade | Erasure; cota menor no celular | **Definido (22)** |
| Papéis de peer | Clientes = nós de msg + histórico | SFU continua só desktop (def. 19) | **Definido (22)** |
| Sharding / erasure por comunidade | Sim, entre os clientes da guild | Pool = membros, não o Chaincord | **Definido** |
| Celular no n do erasure | Sim — todo cliente | Cota pequena; pesa bateria/disco | **Definido (22)** |
| Saída e parcela de storage | Só sai após passar a parcela (escolhe alguém) | Reparo automático no pool + handoff explícito; kick/crash à parte | **Definido (20)** |
| Planos de storage (GB pagos) | Fora de escopo | Disco no pool de membros | **Fechado: não** |
| E2EE vs admin vs histórico vs busca | E2EE; membro novo vê o passado; nó não lê | Canais: E2EE contra nó e estranhos; não contra membros futuros. Busca no servidor continua em tensão | Parcialmente fechado |
| Membro novo vê histórico? | Sim | Ciphertext nos nós + chaves de um membro antigo online | **Definido: sim** |
| Histórico se ninguém antigo está online | Não decifra até alguém ligar | Sem chave = pendente, não vazio fingido | **Definido** |
| Chat ao vivo vs arquivo | Ao vivo independente; arquivo pode pendurar | UI honesta; não misturar perda com indisponível | **Definido (18)** |
| Voz/vídeo 2 clientes | 1:1 sem SFU, inclusive dois celulares | WebRTC + TURN | **Definido (19)** |
| Voz/vídeo 3+ | SFU eleita; ícone cinza se pool vazio | Sem mesh; sem picote | **Definido (19)** |
| Quem não pode ser SFU | Celular, 4G, background, teste falho | Desktop Wi-Fi/Ethernet em foreground | **Definido (19)** |
| UI: quem é a SFU | Ícone no participante (3+) | Sem ícone em 1:1 | **Definido (19)** |
| Stack | Rust + Tauri 2 + React; coturn; OpenMLS | Um core, três papéis (light, storage, SFU) | **Definido (21)** |
| Primeiro device = o servidor | Não | Todos compartilham pedaços; owner ≠ HD | **Definido (22)** |
| Identidade humana (nome) | Pseudônimo ok | `did:key` + alias | Aberto |

---

## Arquitetura atual (revisão — não é definição)

```text
Plano 1  Autoridade         did:key + log assinado (genesis → ops de cargo/canal)
Plano 2  Confidencialidade  MLS nos canais privados; Double Ratchet nos DMs
Plano 3  Disponibilidade    1–3 nós no manifesto da comunidade
Plano 4  Coordenação        STUN/TURN, push, directory opt-in, nós gerenciados
```

Contrato de confiança:

- Integridade / ownership → chaves do usuário.
- Disponibilidade / fan-out → nós da comunidade (podem ser dos usuários).
- Metadados de entrega → o nó vê quem / quando / tamanho, salvo proteção extra depois.

Como descentralizar, em ordem:

1. Protocolo aberto + chaves do usuário.
2. Log de autoridade assinado — o host não concede admin.
3. Mais de um nó por comunidade, escolhidos pelo owner.
4. Cliente local-first — o que você já viu vive no device.
5. Qualquer um pode rodar o nó (a plataforma, se hospedar, é inquilina).
6. Convite carrega `community_id` + endereços + capability.

P2P direto é um **modo** (atalho 1:1, sync de histórico), não a definição da rede. Em CGNAT móvel no Brasil o caminho direto falha com frequência; nós/relays dos usuários continuam sendo descentralização se não forem um operador com poder de protocolo.

---

## Histórico e auditoria

O produto precisa de **histórico por canal**. Sem isso vira IRC: fecha o app, some a conversa.

Onde o histórico vive:

- **Cliente** — cache do que aquele device já viu.
- **Nós da comunidade** — ciphertext para offline e para quem entra depois.
- **Não** no log de autoridade.

Mensagem nova: fan-out ao vivo para quem está conectado. Quem estava offline busca o trecho que falta nos nós. Isso pode *parecer* distribuição por hash (tenho até a 100, me manda 101–140); o swarm é o replica set da comunidade, cifrado, com membership — não um enxame público.

Dois logs:

```text
Log de autoridade     (pequeno)
  genesis, owner, roles, canais, réplicas
  opcional: checkpoint Merkle das pontas dos canais

Log de mensagens      (por canal, arquivável)
  ciphertext assinado pelo autor
  hash(anterior)
  tombstone se houver delete
```

| Objetivo | Corpo no log de autoridade? | Ferramenta |
|---|---|---|
| Mensagem não foi adulterada | Não | Assinatura do autor |
| Ordem do canal não foi reescrita | Não | Hash chain / DAG do canal |
| Admin não apagou em silêncio | Não | Tombstone ou op de delete no log de autoridade |
| Réplica omitiu mensagem | Não | Merkle root periódico no log de autoridade |
| Terceiro lê o que foi dito | Sim, e fere E2EE | Só canal público declarado, ainda assim no log do canal |
| Quem baniu quem | Não | Ops de cargo/kick no log de autoridade |

Membro novo **vê o passado** quando um membro antigo consegue compartilhar as chaves (definições 14 e 15). Fluxo:

1. Join entra no log de autoridade (agora é membro).
2. Cliente baixa ciphertext do canal nos nós — ainda ilegível.
3. O primeiro membro antigo online envia as chaves de histórico (Welcome / history key).
4. Cliente decifra o cache e o canal “aparece”.

Se a comunidade está vazia de gente, o passo 3 não acontece. O produto mostra histórico pendente ou vazio, não quebra o E2EE do nó. Isso é mais Signal que Discord nesse canto; no dia a dia, com alguém online, parece Discord.

MVP de integridade: envelope assinado + hash chain do canal + tombstone. Checkpoint no log de autoridade só se “provar que o host não omitiu” virar requisito.

---

## Armazenamento (definição 17)

Histórico **daquela** comunidade é fatiado entre membros dela. Usuário de outra guild não guarda nada.

```text
Blob cifrado (mídia ou lote de texto)
        │
        ▼
erasure encode  →  n pedaços   (ex.: 10)
        │
        ├── membro A (desktop) guarda mais pedaços
        ├── membro B (PC) guarda mais pedaços
        └── celular             entra no n, cota pequena

Reconstruir: qualquer k pedaços   (ex.: 4 de 10)
Ler o conteúdo: ainda precisa das chaves do canal (membro antigo online)
```

- Texto pode ir inteiro em mais lugares (é barato); **mídia** é o que se fatia.
- Join: cache da janela recente; resto monta sob demanda a partir dos pedaços.
- Membro sai / device some: ver definição 20 (handoff / reparo). Sem isso, k some e o arquivo morre.
- Kick não apaga o que o ex-membro já tinha no disco; só para de receber pedaços novos. Ciphertext sem chave não abre na UI. Reparo no pool restante, sem o chutado escolher destino.
- Cota ainda existe: teto de upload por membro, senão um spammer enche o disco de todo o pool.
- Sem plano pago de GB (definição 16).

**Saída (definição 20):**

```text
Sair (storage peer)
  1. UI: “quem assume seus pedaços?” (membros elegíveis online)
  2. ou reparo automático no resto do pool, se couber
  3. transferência / re-encode termina
  4. só então sai do n (e da comunidade, se for o caso)
```

Se ninguém puder assumir: não completa a saída — avisa que o arquivo da guild fica em risco (definição 18). Sair à força = aceitar essa perda. Vale para desktop e celular (definição 22).

O que isso **não** é: um BitTorrent do Chaincord inteiro. O índice de “quem tem o pedaço X” vive só entre membros da comunidade.

Risco da revisão (ainda válido): celular no *n* pesa disco, bateria e NAT. Mitigação fechada na def. 22: **cota pequena**, não exclusão. SFU continua só desktop (def. 19).

### UI — pendente vs perdido (definição 18)

Não misturar na interface.

| Estado | O que aconteceu | O que a UI diz |
|---|---|---|
| Ao vivo | Alguém no caminho entrega agora | Mensagem normal no canal |
| Pendente — chave | Ciphertext (ou pedaços) existe; ninguém antigo compartilhou chave | “Histórico cifrado, esperando um membro da comunidade” |
| Pendente — k | Pedaços no disco de quem está offline; menos de k online | “Arquivo indisponível até membros voltarem” |
| Perdido | Pedaços abaixo de k e devices sumiram | “Esta parte do histórico não pôde ser recuperada” |

Regras de produto:

- Chat novo não espera o arquivo montar.
- Prefetch / reconstrução em background quando Wi-Fi e storage peers aparecem.
- Não mostrar canal vazio como se nunca tivesse havido conversa, se o cliente sabe que há pedaços ou ciphertext pendente.

---

## Voz, vídeo e tela (definição 19)

SFU = encaminhador seletivo: cada participante sobe **uma** trilha; o hub reenvia. Não mistura mosaic. Não é o log de chat. Não persiste a call.

```text
2 clientes     A ◄──WebRTC──► B     (+ TURN se NAT)     sem SFU
3+ clientes    todos ──► SFU eleita ──► todos
Tela           uma trilha pesada; em grupo, em geral uma por vez
```

**Eleição (só com 3+; automática, zero wizard):**

1. Clientes anunciam capacidade (desktop?, Wi-Fi?, upload estimado, foreground).
2. Elegíveis entram no pool da **aquela call**.
3. Ganha o de melhor upload (desempate: owner da comunidade, depois quem iniciou a sala).
4. Host sai ou satura → reelege no pool; a call pode piscar, não pede IP.
5. Pool vazio → ícone de câmera/tela **de grupo** indisponível. Áudio 1:1 / DM call continua.

**UI da SFU:** badge/ícone no participante eleito (3+). Tooltip simples (“esta pessoa está encaminhando a call”), sem jargão obrigatório na label — o ícone é o sinal; “SFU” pode ficar no tooltip. Troca de host = o badge muda. 2 pessoas: sem badge.

**Não elegível:** mobile, dados móveis, background, teste falho.  
**Elegível:** desktop na call (ou VPS de membro).  
**Fora:** mesh todos-SFU; picotar frames entre SFUs; usuário configurar SFU; gravar call no erasure.

Cascade (2 SFUs + tronco) fica para sala grande, depois — não é o v1 da eleição.

Chat da call segue definição 18: buffer da sala, some quando esvazia.

---

## Stack (definição 21)

Três papéis, um núcleo: cliente leve, desktop storage/SFU, daemon VPS. UI não implementa cripto.

```text
┌─ App desktop (Tauri 2) ───────────────────┐
│  UI React/TS                              │
│  Core Rust: chat · MLS · log · erasure    │
│  opcional: SFU (str0m) · storage peer     │
└───────────────────┬───────────────────────┘
                    │ WS / WebRTC
         ┌──────────┼──────────┐
         ▼          ▼          ▼
    outros cores   coturn    (depois) FCM/APNs
```

| Camada | Escolha |
|---|---|
| Núcleo | Rust |
| UI desktop | Tauri 2 + React/TS |
| Cache local | SQLite (SQLCipher opcional) |
| Transporte chat | WebSocket agora; QUIC (`quinn`) depois |
| Call 1:1 | WebRTC (`str0m` / webrtc-rs) |
| SFU 3+ | `str0m` no binário desktop |
| NAT | coturn (STUN/TURN da plataforma, invisível) |
| Push | FCM / APNs no mobile, depois |
| Erasure | `reed-solomon-erasure` (só desktop/VPS) |
| E2EE grupo | OpenMLS |
| DM | MLS 1:1 ou `vodozemac` |
| Identidade | ed25519-dalek + `did:key` |
| Nó 24h | mesmo binário, `--node` |

**Fora do v1:** Electron; Flutter como core; Go no cliente; LiveKit/mediasoup como produto (servidor que o usuário configuraria); libp2p/DHT; fork Matrix/Element; chaves no renderer JS.

**Ordem:** (1) core: chaves, genesis, log, WS, mensagem ao vivo; (2) SQLite + UI Tauri; (3) MLS + history key; (4) erasure + handoff (def. 20); (5) WebRTC 1:1 + coturn; (6) SFU + eleição + badge; (7) mobile light.

**Plataforma mínima:** coturn; signaling miúdo da call; convites HTTPS. Sem billing de GB (def. 16).

---

## Ideia original (capturada, já filtrada)

Live chat tipo Discord, infra pelos usuários, E2EE, autoridade verificável da comunidade (não do host da plataforma).

Peers conceituais: Light, Relay, Storage. Replicação para churn. NAT, CGNAT, firewall, mobile, WebRTC, STUN, TURN, DHT, relay/fallback. Infra da plataforma só quando necessária.

Comunidade como entidade verificável:

```text
Community
├── #canais
├── Roles
└── Permissions
```

---

## Revisão técnica (2026-08-18)

Tese: **comunidade portátil, host descrente, entrega híbrida.**

**Interessante:** autoridade ≠ infra; identidade = chave; privadas não enumeráveis; réplicas explícitas; P2P quando ambos online; manifesto que sobrevive a troca de host.

**Necessário:** E2EE contra a infra; log assinado; nós persistentes; STUN/TURN; push; convites; RBAC simples; mailbox / histórico offline.

**Adiar:** malha P2P global, DAO, token, storage em DHT de estranhos, voz em mesh.

**MVP (proposta, não definição):** comunidades pequenas, texto+arquivo, canais, roles, host não lê e não rouba owner, self-host e opção hospedada. Identidade, log, nó, MLS, cliente local-first, TURN+push. Fora: token, DAO, DHT global, voz em grupo, directory mundial.

### Respostas condensadas aos 22 pontos

1. Viável como autoridade assinada + nós substituíveis; inviável como mesh mundial no núcleo.
2. P2P: atalho 1:1 e sync entre réplicas designadas.
3. Federado: fan-out, mailbox, presença; plataforma: TURN, push, tenant.
4. Autoridade = log assinado da comunidade.
5. Sem ledger.
6. Identidade: Ed25519 / `did:key`, device linking, seed.
7. E2EE: MLS em canal privado; DM com ratchet; público grande só assinado.
8. Criação: genesis assinado + nó + convite (`id`, addrs, capability).
9. Roles: ops no log; nó e clientes verificam até o genesis.
10. Storage: blobs cifrados no replica set, cache no cliente.
11. Replicação: manifesto com 1–3 nós.
12. Offline: mailbox no nó + push; histórico no nó + cache local.
13. Descoberta: convite; mDNS depois; DHT global adiada.
14. NAT/CGNAT: ICE; hole punch quando der.
15. TURN: caminho de produção — especialmente mobile no Brasil.
16. Peer malicioso: fora do replica set não lê nem forja; no máximo DoS.
17. Sybil/spam: convite + rate limit; sem directory aberto no início.
18. Privacidade: chaves por comunidade; sem membership público.
19. Sem registro público de relações.
20. Escala: particionar por `community_id`.
21. Riscos: escopo, CGNAT, E2EE vs features Discord, liability de blob, voz mesh.
22. MVP: ver acima.

Há um canvas antigo de revisão no projeto Cursor; **este markdown é a fonte das definições.**

---

## Histórico

| Data | O que entrou |
|---|---|
| 2026-08-18 | Primeira captura da ideia (Discord-like, P2P, E2EE, autoridade ≠ host) e revisão dos 22 pontos. |
| 2026-08-18 | Rede **deve** ser descentralizada. Mensagens no log do canal. Sem token obrigatório. |
| 2026-08-18 | Exploração de analogias de ledger **descartada**. Vocabulário e implementação fora de escopo (definição 13). Caderno reescrito sem esses termos. |
| 2026-08-18 | Felipe definiu: **membro novo vê o histórico do canal** (definição 14), porque o produto é Discord descentralizado. Implicação anotada: sem FS contra membros futuros; join com comunidade vazia exige nó servindo history keys ou histórico atrasado. |
| 2026-08-18 | Felipe escolheu a primeira opção: **se não houver membro antigo online, o novo cliente não tem o histórico** (definição 15). Nó não guarda chaves e não lê o canal. |
| 2026-08-18 | Felipe: **sem planos de storage** (definição 16). Primeiro desenho era histórico só nos nós, clientes leves. |
| 2026-08-18 | Felipe redefiniu: **dividir histórico entre membros da comunidade** via sharding/erasure code (definição 17). Pool = aquela guild, não o Chaincord. Revisão: celular continua light; n do erasure são devices que persistem. |
| 2026-08-18 | Felipe fechou: **chat ao vivo não depende de histórico**; arquivo pode ficar pendente; UI alivia sem mentir (definição 18). Offline ≠ perda se o pedaço está em disco; perda só se k não for recuperável. |
| 2026-08-18 | Felipe definiu **eleição automática de uma SFU na call** e **quem não pode ser SFU** (definição 19): celular, 4G, background, teste falho. 1:1 sem SFU. Grupo/tela: ícone indisponível se o pool estiver vazio. |
| 2026-08-18 | Felipe confirmou: **2 clientes na call = sempre 1:1** (sem SFU). 3+ elege SFU. Terceiro a entrar dispara migração. |
| 2026-08-18 | Felipe: na UI da call, **ícone explícito em quem é a SFU** (só 3+; some no 1:1). |
| 2026-08-18 | Felipe: **sair da comunidade (storage) exige passar a parcela** — escolher alguém para assumir (definição 20). Revisão: reparo automático no pool também vale; kick/crash não esperam escolha; light client sai livre. |
| 2026-08-18 | Felipe fechou a **stack do MVP** (definição 21): Rust core, Tauri 2 + React, SQLite, OpenMLS, str0m, coturn. Electron/DHT/Matrix/LiveKit fora. |
| 2026-08-18 | Felipe: **storage não é fixo** (definição 22, primeira versão): owner / primeiro device não é o servidor. Opt-in. |
| 2026-08-18 | Felipe redefiniu 22: **todo cliente é nó de histórico** (não só ao vivo). Celular entra no erasure com cota menor. Handoff vale para todos. |

Próximas anotações: o que o Felipe disser como “é assim” sobe para **Definições**. Perguntas não se anotam, salvo se ele pedir ou fechar uma decisão.
