use crate::data::models::{
    CardMetadata, CardMetadataWithVotes, CardMetadataWithVotesWithoutScore, Pool, UserDTO,
};
use crate::errors::ServiceError;
use crate::errors::DefaultError;
use crate::operators::card_operator::{
    create_openai_embedding, get_card_count_query, get_metadata_from_point_ids,
    insert_card_metadata_query, search_full_text_card_query,
    update_card_html_by_qdrant_point_id_query,
};
use crate::operators::card_operator::{
    get_metadata_from_id_query, get_qdrant_connection, search_card_query,
};
use actix_web::{web, HttpResponse};
use difference::{Changeset, Difference};
use qdrant_client::qdrant::points_selector::PointsSelectorOneOf;
use qdrant_client::qdrant::{PointStruct, PointsIdsList, PointsSelector};
use serde::{Deserialize, Serialize};
use serde_json::json;
use soup::Soup;

use super::auth_handler::LoggedUser;

#[derive(Serialize, Deserialize, Clone)]
pub struct CreateCardData {
    content: String,
    card_html: Option<String>,
    link: Option<String>,
    oc_file_path: Option<String>,
    private: Option<bool>,
}

pub async fn create_card(
    card: web::Json<CreateCardData>,
    pool: web::Data<Pool>,
    user: LoggedUser,
) -> Result<HttpResponse, actix_web::Error> {
    let content_clone = card.content.clone();
    let content_clone_two = card.content.clone();
    let pool_one = pool.clone();
    let pool_two = pool.clone();
    let pool_three = pool.clone();

    let words_in_content = card.content.split(' ').collect::<Vec<&str>>();
    if words_in_content.len() < 70 {
        return Ok(HttpResponse::BadRequest().json(json!({
            "message": "Card content must be at least 70 words long",
        })));
    }

    let check_similarity = |qdrant_point_id: uuid::Uuid, similarity_score: f64, threshold: f64| {
        let lambda_card_content_clone = card.content.clone();
        let card_html_clone = card.card_html.clone();
        let pool_clone = pool.clone();

        let mut similarity_threshold = threshold;
        if lambda_card_content_clone.len() < 200 {
            similarity_threshold -= 0.05;
        }

        if similarity_score >= similarity_threshold {
            let _ = web::block(move || {
                update_card_html_by_qdrant_point_id_query(
                    &qdrant_point_id,
                    &card_html_clone,
                    &pool_clone,
                )
            });

            return Err(DefaultError {
                message: "Card already exists",
            });
        }

        Ok(())
    };

    let pg_similiarity_result = web::block(move || {
        search_full_text_card_query(content_clone, 1, pool_one.clone(), None, None, None)
    })
    .await?
    .map_err(|e| actix_web::error::ErrorBadRequest(e.message))?;

    match pg_similiarity_result.search_results.get(0) {
        Some(result_ref) => {
            match check_similarity(
                result_ref.qdrant_point_id.clone(),
                result_ref.score.unwrap_or(0.0).into(),
                0.9,
            ) {
                Ok(_) => {}
                Err(e) => {
                    return Ok(HttpResponse::BadRequest().json(e));
                }
            }
        }
        None => {}
    }

    let embedding_vector = create_openai_embedding(&content_clone_two).await?;

    let cards = search_card_query(embedding_vector.clone(), 1, pool_two, None, None)
        .await
        .map_err(|e| actix_web::error::ErrorBadRequest(e.message))?;

    match cards.search_results.get(0) {
        Some(result_ref) => {
            match check_similarity(result_ref.point_id.clone(), result_ref.score.into(), 0.95) {
                Ok(_) => {}
                Err(e) => {
                    return Ok(HttpResponse::BadRequest().json(e));
                }
            }
        }
        None => {}
    }

    //if collision is not nil, insert card with collision
    if collision.is_some() {
        web::block(move || {
            insert_duplicate_card_metadata_query(
                CardMetadata::from_details(
                    &card.content,
                    &card.card_html,
                    &card.link,
                    &card.oc_file_path,
                    user.id,
                    None,
                    true,
                ),
                collision.unwrap(),
                &pool,
            )
        })
        .await?
        .map_err(|err| ServiceError::BadRequest(err.message.into()))?;
    } else {
        let payload: qdrant_client::prelude::Payload;
        let qdrant = get_qdrant_connection()
            .await
            .map_err(|err| ServiceError::BadRequest(err.message.into()))?;
        //if private is true, set payload to private
        if private {
            payload = json!({"private": true}).try_into().unwrap();
        } else {
            payload = json!({}).try_into().unwrap();
        }

        let point_id = uuid::Uuid::new_v4();
        let point = PointStruct::new(point_id.clone().to_string(), embedding_vector, payload);

        web::block(move || {
            insert_card_metadata_query(
                CardMetadata::from_details(
                    &card.content,
                    &card.card_html,
                    &card.link,
                    &card.oc_file_path,
                    user.id,
                    Some(point_id),
                    private,
                ),
                &pool,
            )
        })
        .await?
        .map_err(|err| ServiceError::BadRequest(err.message.into()))?;

        qdrant
            .upsert_points_blocking("debate_cards".to_string(), vec![point], None)
            .await
            .map_err(|_err| ServiceError::BadRequest("Failed inserting card to qdrant".into()))?;
    }

    Ok(HttpResponse::NoContent().finish())
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DeleteCardData {
    card_uuid: uuid::Uuid,
}

pub async fn delete_card(
    card: web::Json<DeleteCardData>,
    pool: web::Data<Pool>,
    user: LoggedUser,
) -> Result<HttpResponse, actix_web::Error> {
    let card1 = card.clone();
    let pool1 = pool.clone();
    let card_metadata = web::block(move || get_metadata_from_id_query(card1.card_uuid, pool1))
        .await?
        .map_err(|err| ServiceError::BadRequest(err.message.into()))?;
    if user.id != card_metadata.author_id {
        return Err(ServiceError::Unauthorized.into());
    }
    let qdrant = get_qdrant_connection()
        .await
        .map_err(|err| ServiceError::BadRequest(err.message.into()))?;
    let deleted_values = PointsSelector {
        points_selector_one_of: Some(PointsSelectorOneOf::Points(PointsIdsList {
            ids: vec![card_metadata
                .qdrant_point_id
                .clone()
                .unwrap_or(uuid::Uuid::nil())
                .to_string()
                .into()],
        })),
    };
    web::block(move || delete_card_metadata_query(&card.card_uuid, &pool))
        .await?
        .map_err(|err| ServiceError::BadRequest(err.message.into()))?;

    qdrant
        .delete_points_blocking("debate_cards".to_string(), &deleted_values, None)
        .await
        .map_err(|_err| ServiceError::BadRequest("Failed deleting card from qdrant".into()))?;
    Ok(HttpResponse::NoContent().finish())
}

    let point_id = uuid::Uuid::new_v4();
    let point = PointStruct::new(point_id.clone().to_string(), embedding_vector, payload);
    let card_clone = card.clone();

    web::block(move || {
        insert_card_metadata_query(
            CardMetadata::from_details(
                &card_clone.content,
                &card_clone.card_html,
                &card_clone.link,
                &card_clone.oc_file_path,
                user.id,
                card_metadata.qdrant_point_id,
                private,
            ),
            &pool_three,
        )
    })
    .await?
    .map_err(|err| ServiceError::BadRequest(err.message.into()))?;

    Ok(HttpResponse::NoContent().finish())
}
#[derive(Serialize, Deserialize)]
pub struct SearchCardData {
    content: String,
    filter_oc_file_path: Option<Vec<String>>,
    filter_link_url: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize)]
pub struct ScoreCardDTO {
    metadata: CardMetadataWithVotesWithoutScore,
    score: f32,
}

#[derive(Serialize, Deserialize)]
pub struct SearchCardQueryResponseBody {
    score_cards: Vec<ScoreCardDTO>,
    total_card_pages: i64,
}

pub async fn search_card(
    data: web::Json<SearchCardData>,
    page: Option<web::Path<u64>>,
    user: Option<LoggedUser>,
    pool: web::Data<Pool>,
) -> Result<HttpResponse, actix_web::Error> {
    //search over the links as well
    let page = page.map(|page| page.into_inner()).unwrap_or(1);
    let embedding_vector = create_openai_embedding(&data.content).await?;
    let pool2 = pool.clone();
    let search_card_query_results = search_card_query(
        embedding_vector,
        page,
        pool,
        data.filter_oc_file_path.clone(),
        data.filter_link_url.clone(),
    )
    .await
    .map_err(|err| ServiceError::BadRequest(err.message.into()))?;

    let point_ids = search_card_query_results
        .search_results
        .iter()
        .map(|point| point.point_id)
        .collect::<Vec<_>>();

    let current_user_id = user.map(|user| user.id);
    let metadata_cards =
        web::block(move || get_metadata_from_point_ids(point_ids, current_user_id, pool2))
            .await?
            .map_err(|err| ServiceError::BadRequest(err.message.into()))?;

    let score_cards: Vec<ScoreCardDTO> = search_card_query_results
        .search_results
        .iter()
        .map(|search_result| {
            let card = metadata_cards
                .iter()
                .find(|metadata_card| metadata_card.qdrant_point_id == search_result.point_id)
                .unwrap();

            ScoreCardDTO {
                metadata: <CardMetadataWithVotes as Into<CardMetadataWithVotesWithoutScore>>::into(
                    (*card).clone(),
                ),
                score: search_result.score,
            }
        })
        .collect();

    Ok(HttpResponse::Ok().json(SearchCardQueryResponseBody {
        score_cards,
        total_card_pages: search_card_query_results.total_card_pages,
    }))
}

pub async fn search_full_text_card(
    data: web::Json<SearchCardData>,
    page: Option<web::Path<u64>>,
    user: Option<LoggedUser>,
    pool: web::Data<Pool>,
) -> Result<HttpResponse, actix_web::Error> {
    //search over the links as well
    let page = page.map(|page| page.into_inner()).unwrap_or(1);
    let current_user_id = user.map(|user| user.id);
    let search_results_result = search_full_text_card_query(
        data.content.clone(),
        page,
        pool,
        current_user_id,
        data.filter_oc_file_path.clone(),
        data.filter_link_url.clone(),
    );

    let search_card_query_results = match search_results_result {
        Ok(results) => results,
        Err(err) => return Ok(HttpResponse::BadRequest().json(err)),
    };

    let full_text_cards: Vec<ScoreCardDTO> = search_card_query_results
        .search_results
        .iter()
        .map(|search_result| ScoreCardDTO {
            metadata: <CardMetadataWithVotes as Into<CardMetadataWithVotesWithoutScore>>::into(
                search_result.clone(),
            ),
            score: search_result.score.unwrap_or(0.0),
        })
        .collect();

    Ok(HttpResponse::Ok().json(SearchCardQueryResponseBody {
        score_cards: full_text_cards,
        total_card_pages: search_card_query_results.total_card_pages,
    }))
}

pub async fn get_card_by_id(
    card_id: web::Path<uuid::Uuid>,
    user: Option<LoggedUser>,
    pool: web::Data<Pool>,
) -> Result<HttpResponse, actix_web::Error> {
    let current_user_id = user.map(|user| user.id);
    let card = web::block(move || {
        get_metadata_and_votes_from_id_query(card_id.into_inner(), current_user_id, pool)
    })
    .await?
    .map_err(|err| ServiceError::BadRequest(err.message.into()))?;
    if card.private && current_user_id.is_none() {
        return Ok(HttpResponse::Unauthorized()
            .json(json!({"message": "You must be signed in to view this card"})));
    }
    if card.private && Some(card.clone().author.unwrap().id) != current_user_id {
        return Ok(HttpResponse::Forbidden()
            .json(json!({"message": "You are not authorized to view this card"})));
    }
    Ok(HttpResponse::Ok().json(card))
}

pub async fn get_total_card_count(pool: web::Data<Pool>) -> Result<HttpResponse, actix_web::Error> {
    let total_count = web::block(move || get_card_count_query(&pool))
        .await?
        .map_err(|err| ServiceError::BadRequest(err.message.into()))?;

    Ok(HttpResponse::Ok().json(json!({ "total_count": total_count })))
}
