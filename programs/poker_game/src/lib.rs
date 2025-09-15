use anchor_lang::prelude::*;
use anchor_lang::solana_program::system_instruction;

declare_id!("CEDDEA8Z7kmVL2199EgKMAm4JBYpAPZtCvtnvE1kiaBH");

const MAX_PLAYERS: usize = 6;

#[program]
pub mod poker_game {
    use super::*;

    pub fn initialize_game(
        ctx: Context<InitializeGame>,
        small_blind: u64,
        big_blind: u64,
    ) -> Result<()> {
        let game = &mut ctx.accounts.game;

        game.players = [Pubkey::default(); MAX_PLAYERS];
        game.player_hands = [[0u8; 2]; MAX_PLAYERS];
        game.community_cards = [0u8; 5];
        game.pot = 0;
        game.small_blind = small_blind;
        game.big_blind = big_blind;
        game.current_bet = 0;
        game.current_turn = 0;
        game.betting_round = 0;
        game.is_active = false;
        game.folded = [false; MAX_PLAYERS];
        game.player_bets = [0; MAX_PLAYERS];
        game.players_in_round = 0;

        Ok(())
    }

    pub fn join_game(ctx: Context<JoinGame>, deposit: u64) -> Result<()> {
        let game = &mut ctx.accounts.game;
        let player = &ctx.accounts.player;

        // Prevent joining a full game
        let mut joined = false;

        for i in 0..MAX_PLAYERS {
            if game.players[i] == Pubkey::default() {
                game.players[i] = player.key();
                joined = true;
                game.players_in_round += 1;
                break;
            }
        }

        require!(joined, PokerError::GameFull);

        // Transfer SOL to game pot if deposit > 0
        if deposit > 0 {
            let ix = system_instruction::transfer(&player.key(), &game.key(), deposit);
            anchor_lang::solana_program::program::invoke(
                &ix,
                &[player.to_account_info(), game.to_account_info()],
            )?;
            game.pot += deposit;
        }

        Ok(())
    }

    pub fn start_round(ctx: Context<StartGame>) -> Result<()> {
        let game = &mut ctx.accounts.game;

        require!(!game.is_active, PokerError::GameAlreadyStarted);

        // Shuffle and deal cards
        let clock = Clock::get()?;
        let seed = clock.unix_timestamp as u64 + game.key().to_bytes()[0] as u64;

        let mut deck: Vec<u8> = (0..52).collect();
        pseudo_shuffle(&mut deck, seed);

        // Reset folded and bets
        game.folded = [false; MAX_PLAYERS];
        game.player_bets = [0; MAX_PLAYERS];
        game.pot = 0;

        // Deal hole cards
        let mut deck_index = 0;
        for i in 0..MAX_PLAYERS {
            if game.players[i] != Pubkey::default() {
                game.player_hands[i][0] = deck[deck_index];
                game.player_hands[i][1] = deck[deck_index + 1];
                deck_index += 2;
            }
        }

        // Deal community cards
        for i in 0..5 {
            game.community_cards[i] = deck[deck_index];
            deck_index += 1;
        }

        game.is_active = true;
        game.betting_round = 0;
        game.current_turn = 0;
        game.current_bet = game.big_blind; // Start betting at big blind
        Ok(())
    }

    pub fn bet(ctx: Context<PlayerAction>, amount: u64) -> Result<()> {
        let game = &mut ctx.accounts.game;
        let player = &ctx.accounts.player;

        require!(game.is_active, PokerError::GameNotActive);

        let player_index = game
            .players
            .iter()
            .position(|&p| p == player.key())
            .ok_or(PokerError::PlayerNotInGame)?;

        require!(!game.folded[player_index], PokerError::PlayerFolded);
        require!(player_index as u8 == game.current_turn, PokerError::NotPlayersTurn);

        require!(amount >= game.current_bet, PokerError::BetTooLow);

        game.player_bets[player_index] = amount;
        game.pot += amount;
        game.current_bet = amount;

        // Advance turn
        game.current_turn = next_active_player(&game.players, &game.folded, game.current_turn)?;

        Ok(())
    }

    pub fn call(ctx: Context<PlayerAction>) -> Result<()> {
        let game = &mut ctx.accounts.game;
        let player = &ctx.accounts.player;

        require!(game.is_active, PokerError::GameNotActive);

        let player_index = game
            .players
            .iter()
            .position(|&p| p == player.key())
            .ok_or(PokerError::PlayerNotInGame)?;

        require!(!game.folded[player_index], PokerError::PlayerFolded);
        require!(player_index as u8 == game.current_turn, PokerError::NotPlayersTurn);

        let to_call = game.current_bet.saturating_sub(game.player_bets[player_index]);
        game.player_bets[player_index] += to_call;
        game.pot += to_call;

        // Advance turn
        game.current_turn = next_active_player(&game.players, &game.folded, game.current_turn)?;

        Ok(())
    }

    pub fn fold(ctx: Context<PlayerAction>) -> Result<()> {
        let game = &mut ctx.accounts.game;
        let player = &ctx.accounts.player;

        require!(game.is_active, PokerError::GameNotActive);

        let player_index = game
            .players
            .iter()
            .position(|&p| p == player.key())
            .ok_or(PokerError::PlayerNotInGame)?;

        require!(!game.folded[player_index], PokerError::PlayerAlreadyFolded);
        require!(player_index as u8 == game.current_turn, PokerError::NotPlayersTurn);

        game.folded[player_index] = true;
        game.players_in_round -= 1;

        // Check if only one player remains (winner)
        if game.players_in_round == 1 {
            game.is_active = false;
        } else {
            game.current_turn = next_active_player(&game.players, &game.folded, game.current_turn)?;
        }

        Ok(())
    }

    pub fn reveal_winner(ctx: Context<RevealWinner>, winner: Pubkey) -> Result<()> {
        // Immutable borrow at first
        let game_key = ctx.accounts.game.key();

        // Check game status & winner
        let game = &ctx.accounts.game;

        require!(game.is_active, PokerError::GameNotActive);

        let winner_index = game.players.iter()
            .position(|&p| p == winner)
            .ok_or(PokerError::PlayerNotInGame)?;

        require!(!game.folded[winner_index], PokerError::PlayerFolded);

        // Drop immutable borrow before mutably borrowing lamports
        drop(game);

        // Mutably borrow lamports from game and winner
        let game_account_info = ctx.accounts.game.to_account_info();
        let winner_account_info = ctx.accounts.winner.to_account_info();

        **game_account_info.try_borrow_mut_lamports()? -= ctx.accounts.game.pot;
        **winner_account_info.try_borrow_mut_lamports()? += ctx.accounts.game.pot;

        // Now mutably borrow game to update pot and status
        let game = &mut ctx.accounts.game;
        game.pot = 0;
        game.is_active = false;

        Ok(())
    }
    pub fn end_game(ctx: Context<EndGame>) -> Result<()> {
        // Get AccountInfos first to avoid conflicting borrows
        let game_account_info = ctx.accounts.game.to_account_info();
        let signer_account_info = ctx.accounts.signer.to_account_info();

        // Now get mutable borrow for the game state
        let game = &mut ctx.accounts.game;
        let signer = &ctx.accounts.signer;

        // Authorization check: only first player can end the game
        require!(signer.key() == game.players[0], PokerError::NotAuthorized);
        require!(game.is_active, PokerError::GameNotActive);

        // Refund pot to signer if pot > 0
        if game.pot > 0 {
            **game_account_info.try_borrow_mut_lamports()? -= game.pot;
            **signer_account_info.try_borrow_mut_lamports()? += game.pot;
            game.pot = 0;
        }

        // Reset game state
        game.is_active = false;
        game.players = [Pubkey::default(); MAX_PLAYERS];
        game.player_hands = [[0u8; 2]; MAX_PLAYERS];
        game.community_cards = [0u8; 5];
        game.current_bet = 0;
        game.current_turn = 0;
        game.betting_round = 0;
        game.folded = [false; MAX_PLAYERS];
        game.player_bets = [0; MAX_PLAYERS];
        game.players_in_round = 0;

        Ok(())
    }
}

// Utility function to get next active player's turn
fn next_active_player(players: &[Pubkey; MAX_PLAYERS], folded: &[bool; MAX_PLAYERS], current_turn: u8) -> Result<u8> {
    let mut next = current_turn;
    for _ in 0..MAX_PLAYERS {
        next = (next + 1) % (MAX_PLAYERS as u8);
        if players[next as usize] != Pubkey::default() && !folded[next as usize] {
            return Ok(next);
        }
    }
    Err(PokerError::NoActivePlayers.into())
}

fn pseudo_shuffle(deck: &mut Vec<u8>, seed: u64) {
    let mut state = seed;

    for i in (1..deck.len()).rev() {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let j = (state % (i as u64 + 1)) as usize;
        deck.swap(i, j);
    }
}

#[derive(Accounts)]
pub struct InitializeGame<'info> {
    #[account(init, payer = user, space = 8 + Game::LEN)]
    pub game: Account<'info, Game>,
    #[account(mut)]
    pub user: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct JoinGame<'info> {
    #[account(mut)]
    pub game: Account<'info, Game>,
    #[account(mut)]
    pub player: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct StartGame<'info> {
    #[account(mut)]
    pub game: Account<'info, Game>,
}

#[derive(Accounts)]
pub struct PlayerAction<'info> {
    #[account(mut)]
    pub game: Account<'info, Game>,
    #[account(mut)]
    pub player: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct RevealWinner<'info> {
    #[account(mut)]
    pub game: Account<'info, Game>,

    /// CHECK: This account is not validated by Anchor but is expected to be the winner’s wallet.
    #[account(mut)]
    pub winner: AccountInfo<'info>,
}

#[derive(Accounts)]
pub struct EndGame<'info> {
    #[account(mut)]
    pub game: Account<'info, Game>,

    #[account(mut)]
    pub signer: Signer<'info>,
}


#[account]
pub struct Game {
    pub players: [Pubkey; MAX_PLAYERS],
    pub player_hands: [[u8; 2]; MAX_PLAYERS],
    pub community_cards: [u8; 5],
    pub pot: u64,
    pub small_blind: u64,
    pub big_blind: u64,
    pub current_bet: u64,
    pub current_turn: u8,
    pub betting_round: u8,
    pub is_active: bool,

    pub folded: [bool; MAX_PLAYERS],
    pub player_bets: [u64; MAX_PLAYERS],
    pub players_in_round: u8,
}

impl Game {
    pub const LEN: usize =
        32 * MAX_PLAYERS +    // players: 6 * Pubkey
        2 * MAX_PLAYERS +     // player_hands: 6 * 2 bytes
        5 +                   // community_cards: 5 bytes
        8 +                   // pot
        8 +                   // small_blind
        8 +                   // big_blind
        8 +                   // current_bet
        1 +                   // current_turn
        1 +                   // betting_round
        1 +                   // is_active
        MAX_PLAYERS +         // folded (bool per player)
        8 * MAX_PLAYERS +     // player_bets (u64 per player)
        1;                    // players_in_round
}

#[error_code]
pub enum PokerError {
    #[msg("Game is full.")]
    GameFull,
    #[msg("Game already started.")]
    GameAlreadyStarted,
    #[msg("Game is not active.")]
    GameNotActive,
    #[msg("Player not in game.")]
    PlayerNotInGame,
    #[msg("Player has already folded.")]
    PlayerAlreadyFolded,
    #[msg("Player has folded.")]
    PlayerFolded,
    #[msg("Not player's turn.")]
    NotPlayersTurn,
    #[msg("Bet amount is too low.")]
    BetTooLow,
    #[msg("No active players remaining.")]
    NoActivePlayers,
    #[msg("Not authorized to perform this action.")]
    NotAuthorized,
}
