import * as anchor from "@coral-xyz/anchor";
import { Program, BN } from "@coral-xyz/anchor";
import { PokerGame } from "../target/types/poker_game";
import { Keypair, PublicKey, SystemProgram } from "@solana/web3.js";
import assert from "assert";

/** Map on-chain pubkey to the Keypair we control. Extend for more players. */
function getSignerForPubkey(pk: PublicKey, player1: Keypair, player2: Keypair): Keypair | null {
  if (pk.equals(player1.publicKey)) return player1;
  if (pk.equals(player2.publicKey)) return player2;
  return null;
}

describe("poker_game", () => {
  anchor.setProvider(anchor.AnchorProvider.env());
  const provider = anchor.AnchorProvider.env();
  const program = anchor.workspace.pokerGame as Program<PokerGame>;

  const game = Keypair.generate();
  const player1 = Keypair.generate();
  const player2 = Keypair.generate();

  it("Initializes the game", async () => {
    const smallBlind = new BN(10);
    const bigBlind = new BN(20);

    const tx = await program.methods
      .initializeGame(smallBlind, bigBlind)
      .accounts({
        game: game.publicKey,
        user: provider.wallet.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .signers([game])
      .rpc();

    console.log("Initialize tx:", tx);

    const gameAccount = await program.account.game.fetch(game.publicKey);
    assert.ok(gameAccount.players.every((p) => p.equals(PublicKey.default)));
    assert.ok(gameAccount.pot.eq(new BN(0)));
    assert.equal(gameAccount.smallBlind.toNumber(), 10);
    assert.equal(gameAccount.bigBlind.toNumber(), 20);
  });

  it("Players join the game", async () => {
    // Airdrop SOL to players so they can deposit
    for (const player of [player1, player2]) {
      const sig = await provider.connection.requestAirdrop(player.publicKey, 1_000_000_000);
      await provider.connection.confirmTransaction(sig);
    }

    // Players join with deposit
    for (const player of [player1, player2]) {
      await program.methods
        .joinGame(new BN(1000))
        .accounts({
          game: game.publicKey,
          player: player.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .signers([player])
        .rpc();
    }

    const gameAccount = await program.account.game.fetch(game.publicKey);
    console.log("Seated players:", gameAccount.players.map((p: PublicKey) => p.toString()));
    assert.ok(gameAccount.players[0].equals(player1.publicKey));
    assert.ok(gameAccount.players[1].equals(player2.publicKey));
    assert.ok(gameAccount.playersInRound === 2);
  });

  it("Starts the round", async () => {
    await program.methods
      .startRound()
      .accounts({
        game: game.publicKey,
      })
      .rpc();

    const gameAccount = await program.account.game.fetch(game.publicKey);
    console.log("StartRound - currentTurn:", gameAccount.currentTurn);
    assert.ok(gameAccount.isActive);
    assert.ok(gameAccount.bettingRound === 0);
    assert.ok(typeof gameAccount.currentTurn === "number");
    assert.ok(gameAccount.currentBet.eq(new BN(20)));
  });

  it("Player actions: bet -> call (no fold) -> reveal winner", async () => {
    // 1) Bet: fetch currentTurn and have that player bet
    let gameAccount = await program.account.game.fetch(game.publicKey);
    console.log("Before bet - currentTurn:", gameAccount.currentTurn, "players:", gameAccount.players.map((p: PublicKey) => p.toString()));

    const bettorIndex: number = gameAccount.currentTurn;
    const bettorPubkey: PublicKey = gameAccount.players[bettorIndex];
    const bettor = getSignerForPubkey(bettorPubkey, player1, player2);
    if (!bettor) throw new Error("Unknown bettor signer");

    await program.methods
      .bet(new BN(20))
      .accounts({
        game: game.publicKey,
        player: bettor.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .signers([bettor])
      .rpc();

    gameAccount = await program.account.game.fetch(game.publicKey);
    console.log("After bet - currentTurn:", gameAccount.currentTurn);

    const bettorRecordedIndex = gameAccount.players.findIndex((p: PublicKey) => p.equals(bettor.publicKey));
    assert.ok(gameAccount.playerBets[bettorRecordedIndex].eq(new BN(20)));

    // 2) Call: use the currentTurn signer (whoever the contract says)
    const callerIndex: number = gameAccount.currentTurn;
    const callerPubkey: PublicKey = gameAccount.players[callerIndex];
    const caller = getSignerForPubkey(callerPubkey, player1, player2);
    if (!caller) throw new Error("Unknown caller signer");

    await program.methods
      .call()
      .accounts({
        game: game.publicKey,
        player: caller.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .signers([caller])
      .rpc();

    gameAccount = await program.account.game.fetch(game.publicKey);
    console.log("After call - currentTurn:", gameAccount.currentTurn);

    const callerRecordedIndex = gameAccount.players.findIndex((p: PublicKey) => p.equals(caller.publicKey));
    assert.ok(gameAccount.playerBets[callerRecordedIndex].eq(new BN(20)));

    // Now DON'T fold to keep the game active; instead reveal winner
    // Reveal winner (we use player1 as winner in this test)
    await program.methods
      .revealWinner(player1.publicKey)
      .accounts({
        game: game.publicKey,
        winner: player1.publicKey,
      })
      .rpc();

    gameAccount = await program.account.game.fetch(game.publicKey);
    assert.ok(!gameAccount.isActive, "game should be inactive after revealWinner");
    assert.ok(gameAccount.pot.eq(new BN(0)), "pot expected to be zero after payout");
  });

  // Note: we purposely do not call endGame here because revealWinner already sets is_active=false
});
